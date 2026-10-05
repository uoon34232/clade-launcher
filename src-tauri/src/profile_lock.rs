//! One operation at a time per profile: a profile that is launching or running cannot be prepared
//! or verified, and a profile that is being prepared cannot be launched.
//!
//! Preparation rebuilds files the running game has open (the instance's `natives/`, above all, and
//! on Windows a loaded DLL cannot be replaced), and a launch must never start from half-written
//! files. Both commands claim the profile here **atomically** (one lock, one check-and-insert), so
//! two commands can never both pass separate checks. Different profiles never block each other.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

/// What a profile is currently busy with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// Its game is starting or running (held until the process has ended).
    Launching,
    /// Its files are being prepared, repaired or verified.
    Preparing,
}

#[derive(Default)]
pub struct ProfileLocks {
    inner: Mutex<HashMap<String, Activity>>,
}

impl ProfileLocks {
    fn map(&self) -> MutexGuard<'_, HashMap<String, Activity>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes the profile for `activity`, or says what it is already busy with.
    pub fn claim(&self, id: &str, activity: Activity) -> Result<(), Activity> {
        let mut m = self.map();
        match m.get(id) {
            Some(current) => Err(*current),
            None => {
                m.insert(id.to_owned(), activity);
                Ok(())
            }
        }
    }

    pub fn release(&self, id: &str) {
        self.map().remove(id);
    }

    pub fn current(&self, id: &str) -> Option<Activity> {
        self.map().get(id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn a_profile_is_held_by_one_activity_until_it_is_released() {
        let l = ProfileLocks::default();
        assert_eq!(l.claim("a", Activity::Launching), Ok(()));
        assert_eq!(l.claim("a", Activity::Preparing), Err(Activity::Launching));
        assert_eq!(l.claim("a", Activity::Launching), Err(Activity::Launching));
        l.release("a");
        assert_eq!(l.claim("a", Activity::Preparing), Ok(()));
        assert_eq!(l.claim("a", Activity::Launching), Err(Activity::Preparing));
        assert_eq!(l.current("a"), Some(Activity::Preparing));
    }

    #[test]
    fn other_profiles_are_never_blocked() {
        let l = ProfileLocks::default();
        assert_eq!(l.claim("a", Activity::Launching), Ok(()));
        assert_eq!(l.claim("b", Activity::Preparing), Ok(()));
        assert_eq!(l.claim("c", Activity::Launching), Ok(()));
    }

    #[test]
    fn two_racing_commands_cannot_both_win() {
        // Many rounds of a launch and a prepare released at the same instant: exactly one wins each.
        for _ in 0..200 {
            let l = Arc::new(ProfileLocks::default());
            let gate = Arc::new(Barrier::new(2));
            let spawn = |activity| {
                let (l, gate) = (l.clone(), gate.clone());
                std::thread::spawn(move || {
                    gate.wait();
                    l.claim("p", activity).is_ok()
                })
            };
            let (a, b) = (spawn(Activity::Launching), spawn(Activity::Preparing));
            let wins = usize::from(a.join().unwrap()) + usize::from(b.join().unwrap());
            assert_eq!(wins, 1);
        }
    }
}
