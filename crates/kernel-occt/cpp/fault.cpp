// Keeps a bad read inside OpenCASCADE from ending the app. OCCT's algorithms can walk into a missing edge or past the
// end of a table on parts they can't handle (BRepOffset_Inter2d and BRepOffset_Inter3d do, #91); that is a memory
// fault, which no C++ catch sees. While a thread is inside sk_protect, a read that faults, or anything on the first
// 64 KB of memory (what following a null pointer hits), jumps back here and the operation fails like any other.
// Everything else goes to whoever handled it before: a write to a wild address means damaged memory, so that (and a
// fault in any other code or thread) still stops the app and still leaves a crash report.
//
// What a jump costs: the memory the operation was using is not freed, and its C++ destructors don't run.

#include "fault.hpp"

#include <cstdint>

namespace {
constexpr uintptr_t kNullPage = 0x10000;
}

#ifdef _WIN32

#include <windows.h>

namespace {
int filter(const EXCEPTION_POINTERS* e) {
  const EXCEPTION_RECORD* r = e->ExceptionRecord;
  if (r->ExceptionCode != EXCEPTION_ACCESS_VIOLATION || r->NumberParameters < 2) return EXCEPTION_CONTINUE_SEARCH;
  // 0 is a read, 1 a write, 8 running code that isn't there.
  if (r->ExceptionInformation[0] == 0 || r->ExceptionInformation[1] < kNullPage) return EXCEPTION_EXECUTE_HANDLER;
  return EXCEPTION_CONTINUE_SEARCH;
}
}  // namespace

int sk_protect(int (*run)(void*), void* arg, int* faulted) {
  *faulted = 0;
  __try {
    return run(arg);
  } __except (filter(GetExceptionInformation())) {
    *faulted = 1;
    return 1;
  }
}

#else

#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#if defined(__APPLE__)
#include <sys/ucontext.h>
#else
#include <ucontext.h>
#endif
#if defined(__linux__) && defined(__aarch64__)
#include <asm/sigcontext.h>
#endif

#include <mutex>

namespace {

// A pthread key and not thread_local: reading one in a signal handler never allocates.
pthread_key_t g_jump;
struct sigaction g_before_segv, g_before_bus;
std::once_flag g_once;
bool g_ready = false;

#if defined(__x86_64__)
// A page fault (14) that isn't a write or a jump. A general protection fault (13) is an address no memory can have
// (half of all garbage pointers); it doesn't say which it was, and the access never happened either way.
bool x86_read(unsigned long long trap, unsigned long long error) { return trap == 13 || (trap == 14 && (error & 0x12) == 0); }
#endif

// Whether the fault was a plain read of memory that isn't there, as far as this platform tells.
bool was_read(const void* context) {
  const ucontext_t* uc = static_cast<const ucontext_t*>(context);
  if (uc == nullptr) return false;
#if defined(__APPLE__) && (defined(__aarch64__) || defined(__arm64__))
  const uint32_t esr = uc->uc_mcontext->__es.__esr;
  return (esr >> 26) == 0x24 && (esr & (1u << 6)) == 0;  // a data abort, not a write
#elif defined(__APPLE__) && defined(__x86_64__)
  return x86_read(uc->uc_mcontext->__es.__trapno, uc->uc_mcontext->__es.__err);
#elif defined(__linux__) && defined(__x86_64__)
  return x86_read(uc->uc_mcontext.gregs[REG_TRAPNO], uc->uc_mcontext.gregs[REG_ERR]);
#elif defined(__linux__) && defined(__aarch64__)
  // The fault's syndrome is one of the records behind the registers.
  const unsigned char* at = reinterpret_cast<const unsigned char*>(uc->uc_mcontext.__reserved);
  const unsigned char* const end = at + sizeof(uc->uc_mcontext.__reserved);
  while (at + sizeof(_aarch64_ctx) <= end) {
    const _aarch64_ctx* head = reinterpret_cast<const _aarch64_ctx*>(at);
    if (head->magic == 0 || head->size < sizeof(_aarch64_ctx) || head->size > size_t(end - at)) return false;
    if (head->magic == ESR_MAGIC) {
      const uint64_t esr = reinterpret_cast<const esr_context*>(head)->esr;
      return ((esr >> 26) & 0x3f) == 0x24 && (esr & (1u << 6)) == 0;
    }
    at += head->size;
  }
  return false;
#else
  return false;
#endif
}

void on_fault(int sig, siginfo_t* info, void* context) {
  sigjmp_buf* jump = static_cast<sigjmp_buf*>(pthread_getspecific(g_jump));
  // si_code > 0: a real fault, not a signal somebody sent.
  if (jump != nullptr && info->si_code > 0) {
    bool near_null = reinterpret_cast<uintptr_t>(info->si_addr) < kNullPage;
#if defined(__linux__)
    // x86 reports a non-canonical (wild) address as 0, with a code of its own.
    if (info->si_code != SEGV_MAPERR && info->si_code != SEGV_ACCERR) near_null = false;
#endif
    if (near_null || was_read(context)) siglongjmp(*jump, 1);
  }

  const struct sigaction& before = sig == SIGBUS ? g_before_bus : g_before_segv;
  if (before.sa_flags & SA_SIGINFO) {
    if (before.sa_sigaction != nullptr) {
      before.sa_sigaction(sig, info, context);
      return;
    }
  } else if (before.sa_handler != SIG_DFL && before.sa_handler != SIG_IGN) {
    before.sa_handler(sig);
    return;
  }
  // Nobody else wants it: the same fault again, the default way (a crash report).
  signal(sig, SIG_DFL);
}

void install() {
  if (pthread_key_create(&g_jump, nullptr) != 0) return;
  struct sigaction act = {};
  act.sa_sigaction = on_fault;
  // On the alternate stack, as Rust's own stack overflow handler (called from here) needs; not masked, so jumping
  // out of the handler leaves nothing blocked.
  act.sa_flags = SA_SIGINFO | SA_ONSTACK | SA_NODEFER;
  sigemptyset(&act.sa_mask);
  if (sigaction(SIGSEGV, &act, &g_before_segv) != 0) return;
  if (sigaction(SIGBUS, &act, &g_before_bus) != 0) return;
  g_ready = true;
}

}  // namespace

int sk_protect(int (*run)(void*), void* arg, int* faulted) {
  *faulted = 0;
  std::call_once(g_once, install);
  if (!g_ready) return run(arg);
  sigjmp_buf here;
  void* const outer = pthread_getspecific(g_jump);
  if (sigsetjmp(here, 0) != 0) {
    pthread_setspecific(g_jump, outer);
    *faulted = 1;
    return 1;
  }
  if (pthread_setspecific(g_jump, &here) != 0) return run(arg);
  const int code = run(arg);
  pthread_setspecific(g_jump, outer);
  return code;
}

#endif

int sk_bad_read(void* where) {
  volatile uintptr_t nowhere = reinterpret_cast<uintptr_t>(where);
  return *reinterpret_cast<volatile char*>(nowhere);
}
