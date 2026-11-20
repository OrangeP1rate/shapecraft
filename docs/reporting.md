# Reporting problems and ideas from inside ShapeCraft

**Help ▸ Report a Problem…** and **Help ▸ Suggest an Idea…** open a short form. What you send becomes a public
issue on [ShapeCraft's GitHub](https://github.com/jackharvest/shapecraft/issues), under your own GitHub name, so you
get GitHub's notifications when someone answers. **Help ▸ Your Reports…** lists what you've sent and whether it's
been answered or closed.

When a step fails, its message has a **Report…** button that starts a report about that step. If ShapeCraft
closed unexpectedly, the next start offers one about that.

## What gets sent

Your own words first, then only the details you leave switched on:

| Detail | What it holds |
|---|---|
| What failed | The tool, what it was asked to do (its settings), and the message it gave |
| Crash record | Where ShapeCraft stopped, for a report after it closed unexpectedly |
| Version and system | ShapeCraft's version, the operating system, the graphics card, mouse or touch, the window size |
| Recent log | The last 40 lines of this run's log |
| Screenshot | Copied to your clipboard after sending, to paste into the issue (see below) |

**Preview** shows the exact text before anything leaves. Your home folder is written as `~` in every path. Your
project file is never sent.

## Signing in

The first time you press Send, ShapeCraft shows a short code. Approve it on
[github.com/login/device](https://github.com/login/device) (on this device or any other, a phone works too) and the
report goes out. You stay signed in for next time; **Sign Out** in the form forgets it on this computer.

The sign-in is for the **ShapeCraft Reports** GitHub App. It can only open and read issues on ShapeCraft's own
repository: nothing else of yours, and none of your other repositories. You can revoke it any time under GitHub ▸
Settings ▸ Applications ▸
[Authorized GitHub Apps](https://github.com/settings/apps/authorizations).

No GitHub account? Making one is free, and the sign-in page links to it. Or press **Send from the browser instead**:
GitHub's new-issue form opens with your report filled in.

## The screenshot

GitHub doesn't let apps attach pictures to an issue. So after sending, ShapeCraft puts the screenshot on your
clipboard, and **Open Issue to Paste** takes you to the issue: press ⌘V (Ctrl+V) in the comment box. On phones,
tablets and the tablet's browser page the screenshot row isn't offered.

## For maintainers

Reports carry a hidden marker (`<!-- shapecraft-report v1 … -->`) with the kind, whether a screenshot was offered,
and a signature of a failed step (`fillet · the radius # mm is too large for edge`) so reports of one problem can be
found together. `.github/workflows/app-reports.yml` labels them (`from-app`, `bug` or `enhancement`, the platform,
`needs-screenshot` until a picture arrives). The code is `crates/report` (GitHub calls, the report text) and
`crates/ui-egui/src/report.rs` (the form). The control channel's `ui.report` opens the form for scripts and agents;
nothing is sent without a person pressing Send.
