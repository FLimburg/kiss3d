//! Event handling for DRM windows.

use crate::event::WindowEvent;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

/// Event source for DRM windows
pub enum DrmEventSource {
    /// No event input (default for headless rendering)
    None,

    /// Linux evdev input devices (keyboard, mouse, touchscreen)
    #[cfg(target_os = "linux")]
    Evdev(EvdevEventSource),

    /// Custom event source (user-provided channel)
    Custom(Receiver<WindowEvent>),
}

/// Manager for DRM events
pub struct DrmEventManager {
    source: DrmEventSource,
    /// Accumulated unhandled events from this frame
    unhandled_events: Vec<WindowEvent>,
}

impl DrmEventManager {
    /// Create a new event manager with no input source
    pub fn new_headless() -> Self {
        Self {
            source: DrmEventSource::None,
            unhandled_events: Vec::new(),
        }
    }

    /// Create a new event manager with evdev input
    #[cfg(target_os = "linux")]
    pub fn new_with_evdev(devices: Vec<String>) -> Result<Self, std::io::Error> {
        Ok(Self {
            source: DrmEventSource::Evdev(EvdevEventSource::new(devices)?),
            unhandled_events: Vec::new(),
        })
    }

    /// Create a new event manager with a custom event channel
    pub fn new_with_custom(receiver: Receiver<WindowEvent>) -> Self {
        Self {
            source: DrmEventSource::Custom(receiver),
            unhandled_events: Vec::new(),
        }
    }

    /// Poll for new events (non-blocking)
    pub fn poll_events(&mut self) {
        match &mut self.source {
            DrmEventSource::None => {
                // No events in headless mode
            }
            #[cfg(target_os = "linux")]
            DrmEventSource::Evdev(evdev) => {
                evdev.poll_events(&mut self.unhandled_events);
            }
            DrmEventSource::Custom(receiver) => {
                // Drain all available events from the channel
                loop {
                    match receiver.try_recv() {
                        Ok(event) => self.unhandled_events.push(event),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            log::warn!("Custom event channel disconnected");
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Get all unhandled events and clear the buffer
    pub fn drain_events(&mut self) -> impl Iterator<Item = WindowEvent> + '_ {
        self.unhandled_events.drain(..)
    }

    /// Peek at unhandled events without removing them
    pub fn unhandled_events(&self) -> &[WindowEvent] {
        &self.unhandled_events
    }

    /// Clear all unhandled events
    pub fn clear(&mut self) {
        self.unhandled_events.clear();
    }
}

#[cfg(target_os = "linux")]
pub struct EvdevEventSource {
    _devices: Vec<String>,
}

#[cfg(target_os = "linux")]
impl EvdevEventSource {
    pub fn new(devices: Vec<String>) -> Result<Self, std::io::Error> {
        log::info!("Evdev event source created for devices: {:?}", devices);
        Ok(Self { _devices: devices })
    }

    pub fn poll_events(&mut self, _events: &mut Vec<WindowEvent>) {
        // TODO: Poll evdev devices and convert to WindowEvents
    }
}

/// Helper to create a custom event sender/receiver pair
pub fn create_custom_event_channel() -> (Sender<WindowEvent>, DrmEventManager) {
    let (sender, receiver) = channel();
    let manager = DrmEventManager::new_with_custom(receiver);
    (sender, manager)
}

/// Event manager wrapper for DRM windows that mimics the regular EventManager API
pub struct DrmEventManagerWrapper {
    manager: Rc<RefCell<DrmEventManager>>,
    inhibitor: Rc<RefCell<Vec<WindowEvent>>>,
}

impl DrmEventManagerWrapper {
    pub fn new(manager: Rc<RefCell<DrmEventManager>>) -> Self {
        Self {
            manager,
            inhibitor: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Returns an iterator over events
    pub fn iter(&mut self) -> DrmEventIterator<'_> {
        // Poll for new events
        self.manager.borrow_mut().poll_events();

        DrmEventIterator {
            manager: &self.manager,
            inhibitor: &self.inhibitor,
            index: 0,
        }
    }
}

/// Iterator over DRM events
pub struct DrmEventIterator<'a> {
    manager: &'a RefCell<DrmEventManager>,
    inhibitor: &'a RefCell<Vec<WindowEvent>>,
    index: usize,
}

impl<'a> Iterator for DrmEventIterator<'a> {
    type Item = crate::event::Event<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let manager = self.manager.borrow();
        let events = manager.unhandled_events();

        if self.index < events.len() {
            let event_value = events[self.index];
            self.index += 1;

            // Create an Event using the public constructor
            Some(crate::event::Event::new(event_value, self.inhibitor))
        } else {
            None
        }
    }
}

impl<'a> Drop for DrmEventIterator<'a> {
    fn drop(&mut self) {
        // After iteration, move non-inhibited events to the inhibitor list
        // This mimics the behavior of the regular EventManager
        // The inhibitor list will be used by handle_events
    }
}
