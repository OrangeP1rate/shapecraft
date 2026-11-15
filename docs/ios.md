# ShapeCraft on iPhone (in testing)

ShapeCraft runs as a native app on iPhone, including the iPhone Duo, with the full modeler: the same
OpenCASCADE kernel and the same interface as on a computer, laid out for a phone.

There's no App Store or TestFlight build yet. For now, build it yourself on a Mac and install it on your own phone.

## What's different on a phone

- **Fingers move the view, the stylus works the model.** One finger turns the view, two pan and pinch to zoom, a
  double tap zooms to a face, and a three-finger swipe undoes or redoes. A stylus taps, draws, drags handles, and a
  drag on empty space draws a selection box. The stylus never moves the camera.
- **iPhone Duo.** Folded, the front screen holds your projects (upright). Open it up to model on the inside screen
  (sideways). A project started on the front screen waits with an "Open up to model" card and picks up when you
  unfold. The projects screen fills iOS's vertical bar with its own buttons; in a project, the tools keep clear of the
  status corner.
- **Other iPhones.** Projects show upright, and a project turns the phone sideways.
- **Files.** Import and Export go through the Files app. Projects live in the app's Documents folder, which the
  Files app shows under On My iPhone ▸ ShapeCraft.
- Menus open as iOS sheets, and the interface uses the system font.

## Build and install

You need a Mac with Xcode 27.1 or newer, Rust (`rustup target add aarch64-apple-ios aarch64-apple-ios-sim`), CMake
and Ninja, an OpenCASCADE 7.9 source tree, and an Apple ID signed in to Xcode (a free account works).

```sh
# Your own team id (Xcode ▸ Settings ▸ Accounts) and a bundle id that's yours.
export SHAPECRAFT_IOS_TEAM=ABCDE12345
export SHAPECRAFT_IOS_BUNDLE=com.example.shapecraft

# 1. OpenCASCADE for iOS (once; the phone and the simulator each need their own).
OCCT_SRC=~/src/OCCT-7_9_3 cargo xtask ios-occt device
OCCT_SRC=~/src/OCCT-7_9_3 cargo xtask ios-occt sim

# 2. A signing profile that names your phone (plugged in by USB; UDID from `xcrun devicectl list devices`).
cargo xtask ios-provision <UDID>

# 3. Build, install and start it.
cargo xtask ios-run device <UDID>
# or, in a booted simulator:
cargo xtask ios-run sim <SIMULATOR-UDID>
```

The first time, iOS asks you to trust the developer: Settings ▸ General ▸ VPN & Device Management ▸ your Apple ID ▸
Trust, then open ShapeCraft again.

With a free Apple account, the signing profile lasts 7 days. After that, run step 2 and 3 again to keep using it
(your projects stay).

## Driving the simulator from scripts

A simulator build listens on the control channel (see [mcp.md](mcp.md)) when it's started with
`SIMCTL_CHILD_SHAPECRAFT_CONTROL_PORT=<port>`. The simulator shares the Mac's network, so scripts on the Mac reach it at
`127.0.0.1:<port>`. Builds for a phone never listen.
