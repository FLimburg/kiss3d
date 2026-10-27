# Evdev Input Implementation Plan for DRM Backend

## Overview

This document outlines a complete implementation plan for native evdev input support in the kiss3d DRM backend, enabling keyboard, mouse, and touchscreen input without X11/Wayland.

---

## Current State

### What Works
- ✅ Custom event channel (`set_custom_event_source()`)
- ✅ Users can poll evdev externally and send `WindowEvent`s via channel
- ✅ Event processing pipeline exists and works

### What's Missing
- ❌ Built-in evdev device discovery
- ❌ Automatic event polling
- ❌ Event translation (evdev → WindowEvent)
- ❌ Multi-device handling
- ❌ Hot-plug support

---

## Architecture Design

### High-Level Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Main Thread                              │
│  ┌──────────┐      ┌─────────────────┐                     │
│  │  Window  │◄─────│ DrmEventManager │                     │
│  │          │      │                 │                     │
│  └──────────┘      └─────────────────┘                     │
│                           ▲                                  │
│                           │ WindowEvent                      │
│                           │ (mpsc channel)                   │
└───────────────────────────┼──────────────────────────────────┘
                            │
┌───────────────────────────┼──────────────────────────────────┐
│                 Evdev Polling Thread                         │
│                           │                                  │
│  ┌────────────────────────┴─────────────────────┐          │
│  │         EvdevPoller                           │          │
│  │  ┌──────────────┐  ┌──────────────┐         │          │
│  │  │ Device /dev/ │  │ Device /dev/ │         │          │
│  │  │ input/event0 │  │ input/event1 │  ...    │          │
│  │  │  (keyboard)  │  │   (mouse)    │         │          │
│  │  └──────────────┘  └──────────────┘         │          │
│  │         │                  │                  │          │
│  │         └────────┬─────────┘                 │          │
│  │              EventMapper                      │          │
│  │         (evdev → WindowEvent)                 │          │
│  └───────────────────────────────────────────────┘          │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### Component Breakdown

#### 1. **EvdevPoller** (Background Thread)
- Polls multiple evdev devices using `epoll`
- Runs in dedicated thread to avoid blocking render loop
- Uses non-blocking I/O for responsiveness
- Handles device hotplug via inotify

#### 2. **EventMapper**
- Translates evdev events to kiss3d `WindowEvent`s
- Maintains input state (key press/release, mouse position)
- Handles modifier keys (Ctrl, Alt, Shift)
- Manages relative vs absolute positioning (mouse vs touchscreen)

#### 3. **DeviceManager**
- Auto-discovers input devices from `/dev/input/`
- Filters by device capabilities (keyboard, mouse, touch)
- Opens devices with appropriate permissions
- Handles errors gracefully (permission denied, device busy)

---

## Implementation Plan

### Phase 1: Device Discovery & Opening
**Effort: 2-3 days**

#### Tasks:
1. Add `evdev` crate dependency (optional feature: `drm-evdev`)
2. Implement `DeviceDiscovery::scan_devices()`
   - Read `/dev/input/` directory
   - Query device capabilities via ioctl
   - Filter to keyboard/mouse/touchscreen devices
3. Handle permissions
   - Check if user is in `input` group
   - Provide helpful error messages for permission issues
4. Open devices with error handling
   - Device busy (already grabbed)
   - Permission denied
   - Device disconnected

**Files to create:**
- `src/window/drm/evdev/device_discovery.rs` (~150 lines)
- `src/window/drm/evdev/mod.rs` (~50 lines)

**Key APIs:**
```rust
pub struct DeviceDiscovery;

impl DeviceDiscovery {
    pub fn scan_devices() -> Result<Vec<DeviceInfo>, EvdevError>;
    pub fn open_device(path: &str) -> Result<Device, EvdevError>;
    pub fn filter_by_capability(devices: Vec<DeviceInfo>, cap: Capability) 
        -> Vec<DeviceInfo>;
}

pub struct DeviceInfo {
    pub path: String,
    pub name: String,
    pub capabilities: DeviceCapabilities,
}
```

---

### Phase 2: Event Polling Thread
**Effort: 3-4 days**

#### Tasks:
1. Create background thread with `epoll` for efficient polling
2. Poll multiple devices simultaneously
3. Handle device disconnection gracefully
4. Send raw evdev events to main thread via channel
5. Implement graceful shutdown on window close

**Files to create:**
- `src/window/drm/evdev/poller.rs` (~200 lines)

**Key APIs:**
```rust
pub struct EvdevPoller {
    devices: Vec<Device>,
    event_sender: Sender<RawEvdevEvent>,
    stop_signal: Arc<AtomicBool>,
    thread_handle: Option<JoinHandle<()>>,
}

impl EvdevPoller {
    pub fn new(devices: Vec<Device>, sender: Sender<RawEvdevEvent>) -> Self;
    pub fn start(&mut self) -> Result<(), EvdevError>;
    pub fn stop(&mut self);
}

struct RawEvdevEvent {
    device_id: usize,
    event: evdev::InputEvent,
}
```

**Technical Considerations:**
- Use `epoll` for efficiency (only wake when events available)
- Timeout of ~16ms to check stop signal (60fps responsiveness)
- Handle `EAGAIN`/`EWOULDBLOCK` properly
- Log device errors but don't crash thread

---

### Phase 3: Event Translation (Keyboard)
**Effort: 2-3 days**

#### Tasks:
1. Map evdev key codes to kiss3d `Key` enum
2. Handle key press/release/repeat
3. Track modifier state (Ctrl, Alt, Shift, Super)
4. Generate `Char` events for printable keys
5. Handle key combinations

**Files to create:**
- `src/window/drm/evdev/keyboard_mapper.rs` (~250 lines)
- `src/window/drm/evdev/keymap.rs` (~300 lines - key code table)

**Key APIs:**
```rust
pub struct KeyboardMapper {
    modifier_state: Modifiers,
    pressed_keys: HashSet<Key>,
}

impl KeyboardMapper {
    pub fn process_event(&mut self, event: evdev::InputEvent) 
        -> Option<WindowEvent>;
    fn map_keycode(code: u16) -> Option<Key>;
    fn get_char_for_key(key: Key, modifiers: Modifiers) -> Option<char>;
}
```

**Mapping Complexity:**
- ~100 keys to map (letters, numbers, F-keys, etc.)
- Handle locale/keymap (start with US layout)
- Compose keys (dead keys) - defer to later
- Numpad handling

---

### Phase 4: Event Translation (Mouse)
**Effort: 2-3 days**

#### Tasks:
1. Handle relative mouse movement (standard mouse)
2. Handle absolute positioning (touchpad, tablet)
3. Track cursor position
4. Map mouse buttons (left, right, middle, extra)
5. Handle scroll wheel (vertical & horizontal)

**Files to create:**
- `src/window/drm/evdev/mouse_mapper.rs` (~200 lines)

**Key APIs:**
```rust
pub struct MouseMapper {
    cursor_pos: (f64, f64),
    screen_size: (u32, u32),
    button_state: [bool; 8],
}

impl MouseMapper {
    pub fn process_event(&mut self, event: evdev::InputEvent) 
        -> Option<WindowEvent>;
    fn handle_relative_motion(&mut self, dx: i32, dy: i32);
    fn handle_absolute_motion(&mut self, x: i32, y: i32, max_x: i32, max_y: i32);
    fn handle_button(&mut self, button: u16, pressed: bool) -> WindowEvent;
    fn handle_wheel(&mut self, axis: Axis, delta: i32) -> WindowEvent;
}
```

**Technical Considerations:**
- Relative: accumulate dx/dy, clamp to screen bounds
- Absolute: normalize coordinates to screen size
- Smooth scrolling vs discrete scroll events
- Handle inverted axes (natural scrolling)

---

### Phase 5: Event Translation (Touch)
**Effort: 2-3 days**

#### Tasks:
1. Handle multi-touch (tracking points by slot/id)
2. Map to kiss3d `Touch` events
3. Handle touch pressure (if available)
4. Distinguish touch vs stylus

**Files to create:**
- `src/window/drm/evdev/touch_mapper.rs` (~200 lines)

**Key APIs:**
```rust
pub struct TouchMapper {
    active_touches: HashMap<u64, TouchPoint>,
    screen_size: (u32, u32),
}

impl TouchMapper {
    pub fn process_event(&mut self, event: evdev::InputEvent) 
        -> Vec<WindowEvent>;
    fn handle_touch_down(&mut self, id: u64, x: f64, y: f64);
    fn handle_touch_move(&mut self, id: u64, x: f64, y: f64);
    fn handle_touch_up(&mut self, id: u64);
}
```

**Technical Considerations:**
- MT protocol A vs B (use protocol B - slot-based)
- Touch ID management
- Coordinate normalization
- Distinguish palm rejection events

---

### Phase 6: Integration & Configuration
**Effort: 2-3 days**

#### Tasks:
1. Wire up all components
2. Add configuration options:
   - Auto-discover devices vs explicit paths
   - Filter by device type
   - Grab devices (exclusive access)
3. Update `Window::enable_evdev_input()` implementation
4. Error handling and logging
5. Documentation

**Files to modify:**
- `src/window/drm/drm_window.rs` (~50 lines changed)
- `src/window/drm/drm_events.rs` (~100 lines changed)

**Configuration API:**
```rust
pub struct EvdevConfig {
    /// Auto-discover devices (default: true)
    pub auto_discover: bool,
    
    /// Explicit device paths (if not auto-discovering)
    pub device_paths: Vec<String>,
    
    /// Grab devices for exclusive access (default: false)
    pub grab_devices: bool,
    
    /// Enable keyboard input (default: true)
    pub enable_keyboard: bool,
    
    /// Enable mouse input (default: true)
    pub enable_mouse: bool,
    
    /// Enable touch input (default: true)
    pub enable_touch: bool,
}

impl Window {
    pub fn enable_evdev_input(&mut self) -> Result<(), std::io::Error>;
    pub fn enable_evdev_input_with_config(&mut self, config: EvdevConfig) 
        -> Result<(), std::io::Error>;
}
```

---

### Phase 7: Testing & Polish
**Effort: 2-3 days**

#### Tasks:
1. Unit tests for event mappers
2. Integration test with mock devices
3. Manual testing on real hardware:
   - Raspberry Pi
   - Desktop Linux (USB keyboard/mouse)
   - Laptop (touchpad)
   - Touchscreen device
4. Performance testing (input latency)
5. Fix bugs and edge cases
6. Documentation examples

**Test Scenarios:**
- Single keyboard input
- Single mouse input
- Keyboard + mouse simultaneously
- Multi-touch gestures
- Device disconnect/reconnect
- Permission denied handling
- Device busy handling
- High event rate (rapid key repeat, fast mouse movement)

---

## Dependencies

### New Crate Dependencies
```toml
[dependencies.evdev]
version = "0.12"
optional = true

[features]
drm-evdev = ["drm", "evdev"]
```

### System Requirements
- Linux kernel with evdev support (virtually all modern kernels)
- User must be in `input` group OR run as root (document this)
- `/dev/input/` must be accessible

---

## Effort Estimation

### Time Breakdown

| Phase | Component | Lines of Code | Estimated Time |
|-------|-----------|---------------|----------------|
| 1 | Device Discovery | ~200 | 2-3 days |
| 2 | Polling Thread | ~200 | 3-4 days |
| 3 | Keyboard Mapping | ~550 | 2-3 days |
| 4 | Mouse Mapping | ~200 | 2-3 days |
| 5 | Touch Mapping | ~200 | 2-3 days |
| 6 | Integration | ~150 | 2-3 days |
| 7 | Testing & Polish | - | 2-3 days |
| **Total** | | **~1,500 LOC** | **15-22 days** |

### Effort by Skill Level

| Skill Level | Total Effort |
|-------------|--------------|
| **Experienced Rust + Linux developer** | 15-18 days |
| **Intermediate Rust developer** | 20-25 days |
| **Junior developer (learning as you go)** | 30-40 days |

### Risk Factors (Could Add Time)

- **Platform quirks**: +20% (some devices have weird behavior)
- **Permission issues**: +10% (SELinux, AppArmor complications)
- **Testing on diverse hardware**: +20% (Raspberry Pi, various laptops)
- **Documentation**: +15% (comprehensive examples, troubleshooting guide)

**Realistic estimate with contingency: 20-30 working days (4-6 weeks calendar time)**

---

## Complexity Rating

### Low Complexity ⭐⭐
- Device discovery (straightforward file I/O)
- Basic key mapping (lookup table)

### Medium Complexity ⭐⭐⭐
- Polling thread (thread management, epoll)
- Mouse event handling (coordinate transformation)
- Error handling (many failure modes)

### High Complexity ⭐⭐⭐⭐
- Multi-touch tracking (state machine, slot management)
- Modifier key combinations (state tracking)
- Character generation with modifiers (shift, caps lock, numlock)
- Hotplug support (inotify, dynamic device addition)

---

## Alternatives to Full Implementation

### Option 1: Minimal Implementation (5-7 days)
**Scope**: Keyboard-only, no auto-discovery
- Manual device path specification
- Basic key press/release
- No char events, no modifiers
- ~400 LOC

**Pros**: Quick to implement, covers basic use cases
**Cons**: Limited functionality, still requires user configuration

### Option 2: Partial Implementation (10-15 days)
**Scope**: Keyboard + mouse, auto-discovery
- Auto-discover keyboard and mouse
- Full keyboard mapping with modifiers
- Mouse movement and buttons (no scroll)
- ~800 LOC

**Pros**: Covers 80% of use cases
**Cons**: No touch support, no scroll wheel

### Option 3: External Library (0 days implementation, maintenance overhead)
**Use existing input library**: `winit` (but requires X11/Wayland) or `smithay-client-toolkit`

**Pros**: No implementation time
**Cons**: Heavy dependencies, may not fit DRM-only use case

---

## Recommended Approach

### Phase 1: Start with Option 2 (Partial Implementation)
- Implement keyboard + mouse only
- Auto-discovery for ease of use
- Full event translation
- ~10-15 days effort

### Phase 2: Add Touch Later (Optional)
- Add touch support when needed
- ~+5 days effort
- Can be separate PR/feature flag

### Why This Makes Sense:
1. **Keyboard + mouse covers 95% of embedded DRM use cases**
2. **Touchscreen support adds significant complexity for rare use case**
3. **Users have working alternative (custom event channel)**
4. **Can iterate based on user feedback**

---

## Migration Path for Users

### Current Code (Custom Channel)
```rust
let (sender, receiver) = std::sync::mpsc::channel();
window.set_custom_event_source(receiver);

// User implements their own evdev polling
std::thread::spawn(move || {
    // Poll /dev/input/event0, map to WindowEvent, send via sender
});
```

### After Implementation
```rust
// Simple case - auto-discovery
window.enable_evdev_input()?;

// Advanced case - custom config
window.enable_evdev_input_with_config(EvdevConfig {
    device_paths: vec!["/dev/input/event0".to_string()],
    grab_devices: true,
    ..Default::default()
})?;
```

---

## Success Criteria

### Must Have (MVP)
- ✅ Auto-discover keyboard and mouse devices
- ✅ Translate keyboard events (key press/release, modifiers, char events)
- ✅ Translate mouse events (movement, buttons, scroll)
- ✅ Handle device disconnection gracefully
- ✅ Clear error messages for permission issues
- ✅ Zero-config experience for common setups

### Should Have
- ✅ Multi-device support (multiple keyboards/mice)
- ✅ Configurable device filtering
- ✅ Comprehensive documentation
- ✅ Example programs

### Nice to Have
- ⭐ Touch support
- ⭐ Hotplug support (device add/remove while running)
- ⭐ Device grab mode (exclusive access)
- ⭐ Configurable key repeat rate

---

## Conclusion

**Full implementation: 20-30 working days**  
**Recommended partial implementation: 10-15 working days**

The evdev implementation is moderately complex but well-scoped. The recommended approach is to implement keyboard + mouse support first (Option 2), which covers the vast majority of DRM use cases while keeping complexity manageable.

Given that users currently have a working alternative (`set_custom_event_source()`), this is not a critical blocker and can be implemented when resources are available or when there's clear user demand.
