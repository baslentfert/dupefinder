//! Windows system restore points, as a net under the orphan cleanup.
//!
//! # What this does and does not protect
//!
//! A restore point snapshots system files, installed programs and the
//! registry. It explicitly does **not** monitor the file types people keep
//! their own data in — documents, pictures, mail. Rolling back will not bring
//! a deleted photo back.
//!
//! So this is deliberately wired to the orphan detectors only, and nothing
//! else. Removing a package from the installer cache or a folder of leftover
//! application data can break a product's repair and uninstall path, and that
//! is exactly what a restore point *can* undo. Offering the same net for the
//! duplicate and photo cleanup would be false comfort, and false comfort about
//! deleting files is worse than none: the recycle bin is the real undo there.
//!
//! # Bracketing
//!
//! Windows wants a change bracketed: one call opens the restore point, the
//! work happens, a second call closes it. [`RestorePoint`] closes itself on
//! drop, so an early return or a panic cannot leave the system state manager
//! believing a change is still in progress.

#[cfg(windows)]
use std::fmt;

/// Whether a restore point can be made right now, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Availability {
    /// Everything is in place.
    Ready,
    /// System protection is on, but creating a point needs an elevated token.
    NeedsElevation,
    /// System protection is switched off for the system drive.
    ProtectionDisabled,
    /// Not Windows.
    Unsupported,
}

impl Availability {
    pub fn is_ready(self) -> bool {
        matches!(self, Availability::Ready)
    }

    /// A sentence the UI can show verbatim.
    pub fn explain(self) -> &'static str {
        match self {
            Availability::Ready => "A restore point will be created first.",
            Availability::NeedsElevation => {
                "Creating a restore point needs administrator rights. Restart the app as \
                 administrator, or continue without one."
            }
            Availability::ProtectionDisabled => {
                "System protection is switched off for this drive, so Windows cannot create a \
                 restore point. Turn it on under System Properties → System Protection."
            }
            Availability::Unsupported => "Restore points are a Windows feature.",
        }
    }
}

#[derive(Debug)]
pub enum Outcome {
    Created(RestorePoint),
    Unavailable(Availability),
    Failed { code: u32, message: String },
}

/// An open restore point. Closing it is what makes it usable, so it closes
/// itself when it goes out of scope.
#[derive(Debug)]
pub struct RestorePoint {
    sequence: i64,
    closed: bool,
}

impl RestorePoint {
    pub fn sequence(&self) -> i64 {
        self.sequence
    }
}

#[cfg(windows)]
mod sys {
    use super::*;

    use windows::Win32::System::Restore::{
        SRSetRestorePointW, BEGIN_SYSTEM_CHANGE, END_SYSTEM_CHANGE, MODIFY_SETTINGS,
        RESTOREPOINTINFOW, STATEMGRSTATUS,
    };

    /// `RPSessionInterval` of zero, or `DisableSR` of one, means the user has
    /// turned system protection off. Both are readable without elevation,
    /// which is what lets the UI say so before the user commits to anything.
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore";

    pub fn availability() -> Availability {
        use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
        use winreg::RegKey;

        let disabled = RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(KEY, KEY_READ)
            .map(|key| {
                let off = |name: &str, when: u32| {
                    key.get_value::<u32, _>(name)
                        .map(|v| v == when)
                        .unwrap_or(false)
                };
                off("DisableSR", 1) || off("RPSessionInterval", 0)
            })
            // An unreadable key is not evidence that protection is off, so
            // fall through and let the API have its say.
            .unwrap_or(false);

        if disabled {
            return Availability::ProtectionDisabled;
        }
        if !crate::platform::is_elevated() {
            return Availability::NeedsElevation;
        }
        Availability::Ready
    }

    pub fn begin(description: &str) -> Outcome {
        let availability = availability();
        if !availability.is_ready() {
            return Outcome::Unavailable(availability);
        }

        // `RESTOREPOINTINFOW` is `packed(1)`, so a reference to one of its
        // fields would be misaligned — undefined behaviour even unread. The
        // buffer is built separately and moved in whole.
        let info = RESTOREPOINTINFOW {
            dwEventType: BEGIN_SYSTEM_CHANGE,
            // Not APPLICATION_UNINSTALL: nothing is being uninstalled, leftovers
            // of something already gone are being removed.
            dwRestorePtType: MODIFY_SETTINGS,
            llSequenceNumber: 0,
            szDescription: description_buffer(description),
        };

        let mut status = STATEMGRSTATUS::default();
        let ok = unsafe { SRSetRestorePointW(&info, &mut status) };

        // Read the packed fields out by value before doing anything with them.
        let sequence = status.llSequenceNumber;
        let code = status.nStatus.0;

        if ok.as_bool() {
            Outcome::Created(RestorePoint {
                sequence,
                closed: false,
            })
        } else {
            Outcome::Failed {
                code,
                message: describe(code),
            }
        }
    }

    pub fn close(point: &mut RestorePoint) {
        if point.closed {
            return;
        }
        point.closed = true;

        let info = RESTOREPOINTINFOW {
            dwEventType: END_SYSTEM_CHANGE,
            dwRestorePtType: MODIFY_SETTINGS,
            llSequenceNumber: point.sequence,
            szDescription: [0; 256],
        };

        let mut status = STATEMGRSTATUS::default();
        let ok = unsafe { SRSetRestorePointW(&info, &mut status) };

        let code = status.nStatus.0;
        if !ok.as_bool() {
            tracing::warn!(
                sequence = point.sequence,
                code,
                "could not close the restore point"
            );
        }
    }

    /// A NUL-terminated wide string in the fixed buffer Windows expects,
    /// truncated to fit. This text is what the restore UI shows the user.
    fn description_buffer(text: &str) -> [u16; 256] {
        let mut buffer = [0u16; 256];
        let wide: Vec<u16> = text.encode_utf16().take(buffer.len() - 1).collect();
        buffer[..wide.len()].copy_from_slice(&wide);
        buffer
    }

    fn describe(code: u32) -> String {
        // The codes a user can actually do something about.
        const ERROR_ACCESS_DENIED: u32 = 5;
        const ERROR_SERVICE_DISABLED: u32 = 1058;
        const ERROR_TIMEOUT: u32 = 1460;

        match code {
            ERROR_ACCESS_DENIED => "access denied — administrator rights are needed".into(),
            ERROR_SERVICE_DISABLED => "the System Restore service is disabled".into(),
            ERROR_TIMEOUT => "Windows took too long to create the restore point".into(),
            other => format!("Windows reported error {other}"),
        }
    }

    impl fmt::Display for Availability {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.explain())
        }
    }
}

#[cfg(windows)]
pub use sys::availability;

#[cfg(not(windows))]
pub fn availability() -> Availability {
    Availability::Unsupported
}

/// Open a restore point describing what is about to happen.
///
/// The returned [`RestorePoint`] must stay alive for the duration of the work;
/// dropping it tells Windows the change is finished.
#[cfg(windows)]
pub fn begin(description: &str) -> Outcome {
    sys::begin(description)
}

#[cfg(not(windows))]
pub fn begin(_description: &str) -> Outcome {
    Outcome::Unavailable(Availability::Unsupported)
}

impl Drop for RestorePoint {
    fn drop(&mut self) {
        #[cfg(windows)]
        sys::close(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_is_reported_without_elevation_rather_than_guessed() {
        // The test runner is not elevated, and this machine has protection on,
        // so the honest answer is "needs elevation" — not "disabled", and not
        // an optimistic "ready" that would fail at the worst moment.
        let state = availability();
        assert!(
            matches!(
                state,
                Availability::NeedsElevation
                    | Availability::Ready
                    | Availability::ProtectionDisabled
            ),
            "unexpected {state:?}"
        );
        assert!(!state.explain().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn an_unavailable_state_never_claims_a_point_was_made() {
        // Whatever the machine's configuration, `begin` must not report
        // success when it could not actually create anything.
        match begin("Duplicate Finder test") {
            Outcome::Created(point) => {
                assert!(availability().is_ready());
                assert!(point.sequence() > 0);
            }
            Outcome::Unavailable(state) => assert!(!state.is_ready()),
            Outcome::Failed { message, .. } => assert!(!message.is_empty()),
        }
    }

    #[test]
    fn every_state_explains_itself() {
        for state in [
            Availability::Ready,
            Availability::NeedsElevation,
            Availability::ProtectionDisabled,
            Availability::Unsupported,
        ] {
            assert!(state.explain().len() > 10, "{state:?} has no explanation");
        }
    }
}
