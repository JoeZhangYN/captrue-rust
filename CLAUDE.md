# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

### Build and Run
- **Build**: `cargo build` (debug) or `cargo build --release` (optimized)
- **Run**: `cargo run` or `cargo run --release`
- **Lint**: `cargo clippy` - Run the Rust linter to catch common mistakes
- **Format**: `cargo fmt` - Format code according to Rust standards

### Testing
- **Run all tests**: `cargo test`
- **Run specific test**: `cargo test <test_name>`
- **Run tests with output**: `cargo test -- --nocapture`

## Architecture

This is a Windows-specific screen capture tool written in Rust that allows users to capture and save screen regions with precise positioning information encoded in filenames.

### Core Components

1. **Event System** (`AppEvent` enum): Handles keyboard, mouse, and global hotkey events through a message-passing architecture between threads.

2. **State Machine** (`State` enum): Manages application flow through distinct states:
   - `Idle`: Waiting for capture hotkey
   - `FullscreenCapture`: Displaying captured screen
   - `SelectingRegion`: User dragging to select red box region
   - `RegionSelected`: Red box selected, can save or select sub-region
   - `SelectingSubRegion`: User selecting green box within red box
   - `SubRegionSelected`: Both boxes selected, ready to save

3. **Hotkey System**:
   - Uses Windows API (`RegisterHotKey`) running in separate thread
   - Ctrl+Alt+D: Triggers screen capture
   - Ctrl+S: Saves selected region
   - ESC: Cancels/goes back one state

4. **Image Processing**:
   - Captures using `screenshots` crate
   - Saves as WebP format (lossless) using `webp` crate
   - Filenames encode position and size: `W{screen_width}H{screen_height}/screenshot_{timestamp}_Lx{x}Ty{y}W{width}H{height}.webp`

5. **Display System**:
   - Uses `minifb` for borderless fullscreen window
   - Optimized rendering with buffer reuse
   - Gray overlay for unselected areas with colored selection boxes (red for main, green for sub-region)

### Key Dependencies
- `screenshots`: Screen capture functionality
- `minifb`: Minimal framebuffer window management
- `image`: Image processing operations
- `webp`: WebP encoding for efficient lossless compression
- `winapi`/`windows`: Windows API integration for hotkeys and window management

### Performance Optimizations
- Buffer reuse in `display_image` function to reduce memory allocations
- 60 FPS frame rate limiting
- Efficient grayscale overlay using direct pixel manipulation
- Thread separation for hotkey monitoring vs main event loop