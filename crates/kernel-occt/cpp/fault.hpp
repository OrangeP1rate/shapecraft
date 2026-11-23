#pragma once

// Runs `run(arg)`; a read of memory that isn't there inside it makes this return 1 with *faulted = 1 (see fault.cpp).
// `run` must let no C++ exception out.
int sk_protect(int (*run)(void*), void* arg, int* faulted);

// Reads from an address (for the test of the above).
int sk_bad_read(void* where);

// What an operation that ended in a fault says.
constexpr const char* kFailed = "the geometry kernel failed on this shape";
