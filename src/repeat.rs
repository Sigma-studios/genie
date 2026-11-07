//! Key repeat. Wayland leaves it to the client, and wayapp doesn't ask for it,
//! so without this a held Backspace deletes one character.

use smithay_client_toolkit::seat::keyboard::KeyEvent;
use std::time::Duration;
use std::time::Instant;

/// The compositor's defaults, and COSMIC's here: 600 ms, then 25 a second.
const DELAY: Duration = Duration::from_millis(600);
const INTERVAL: Duration = Duration::from_millis(40);

/// Which window the held key was pressed in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Launcher,
    List,
}

struct Held {
    key: KeyEvent,
    target: Target,
    next: Instant,
}

#[derive(Default)]
pub struct Repeat {
    held: Option<Held>,
}

impl Repeat {
    /// The last key pressed repeats, as in any text field; modifiers don't.
    pub fn press(&mut self, key: &KeyEvent, target: Target, now: Instant) {
        if key.keysym.is_modifier_key() {
            return;
        }
        self.held = Some(Held { key: key.clone(), target, next: now + DELAY });
    }

    pub fn release(&mut self, key: &KeyEvent) {
        if self.held.as_ref().is_some_and(|held| held.key.raw_code == key.raw_code) {
            self.held = None;
        }
    }

    /// Focus lost or the window gone: the release won't come.
    pub fn stop(&mut self) {
        self.held = None;
    }

    pub fn next(&self) -> Option<Instant> {
        self.held.as_ref().map(|held| held.next)
    }

    /// The key to repeat, if it's time. One at a time: after a stall the next
    /// comes an interval later, not as a burst.
    pub fn due(&mut self, now: Instant) -> Option<(KeyEvent, Target)> {
        let held = self.held.as_mut().filter(|held| held.next <= now)?;
        held.next = now + INTERVAL;
        Some((held.key.clone(), held.target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay_client_toolkit::seat::keyboard::Keysym;

    fn key(raw_code: u32, keysym: Keysym) -> KeyEvent {
        KeyEvent { time: 0, raw_code, keysym, utf8: None }
    }

    #[test]
    fn a_held_key_repeats_after_the_delay_until_released() {
        let start = Instant::now();
        let mut repeat = Repeat::default();
        let backspace = key(14, Keysym::BackSpace);
        repeat.press(&backspace, Target::Launcher, start);
        assert!(repeat.due(start + DELAY / 2).is_none());
        let (repeated, target) = repeat.due(start + DELAY).unwrap();
        assert_eq!((repeated.raw_code, target), (14, Target::Launcher));
        assert!(repeat.due(start + DELAY).is_none());
        assert!(repeat.due(start + DELAY + INTERVAL).is_some());
        repeat.release(&backspace);
        assert!(repeat.next().is_none());
    }

    #[test]
    fn modifiers_dont_repeat_or_stop_the_held_key() {
        let start = Instant::now();
        let mut repeat = Repeat::default();
        repeat.press(&key(30, Keysym::a), Target::List, start);
        repeat.press(&key(42, Keysym::Shift_L), Target::List, start);
        repeat.release(&key(42, Keysym::Shift_L));
        assert_eq!(repeat.due(start + DELAY).map(|(k, _)| k.raw_code), Some(30));
    }
}
