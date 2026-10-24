use crate::{Effect, Status};

/// Semantic classification is independent of the diagnostic/wire code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureKind {
    Operational,
    Unsupported,
    Expired,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub effect: Effect,
    /// Optional numeric operating-system evidence; never a raw message or credential.
    pub os_error: Option<i32>,
    kind: FailureKind,
}
impl Failure {
    const fn classified(code: &'static str, kind: FailureKind) -> Self {
        Self {
            code,
            kind,
            effect: Effect::None,
            os_error: None,
        }
    }

    /// A pre-dispatch operational failure. Only the three historical reserved
    /// codes retain implicit classification; new codes must use explicit constructors.
    pub const fn before(code: &'static str) -> Self {
        let kind = match code.as_bytes() {
            b"unsupported" => FailureKind::Unsupported,
            b"expired" => FailureKind::Expired,
            b"cancelled" => FailureKind::Cancelled,
            _ => FailureKind::Operational,
        };
        Self::classified(code, kind)
    }

    pub const fn unsupported(code: &'static str) -> Self {
        Self::classified(code, FailureKind::Unsupported)
    }
    pub const fn expired(code: &'static str) -> Self {
        Self::classified(code, FailureKind::Expired)
    }
    pub const fn cancelled(code: &'static str) -> Self {
        Self::classified(code, FailureKind::Cancelled)
    }
    pub const fn uncertain(code: &'static str) -> Self {
        Self::before(code).after_dispatch()
    }
    /// Preserve diagnostic evidence and classification when an effect is possible.
    pub const fn after_dispatch(mut self) -> Self {
        self.effect = Effect::MayHaveChanged;
        self
    }
    pub const fn with_os_error(mut self, code: Option<i32>) -> Self {
        self.os_error = code;
        self
    }
    /// Uncertainty always wins, including over deadline/cancellation failures.
    pub const fn status(&self) -> Status {
        if matches!(self.effect, Effect::MayHaveChanged) {
            return Status::OutcomeUnknown;
        }
        match self.kind {
            FailureKind::Operational => Status::Failed,
            FailureKind::Unsupported => Status::Unsupported,
            FailureKind::Expired => Status::Expired,
            FailureKind::Cancelled => Status::Cancelled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_classification_does_not_depend_on_code_spelling() {
        for (failure, expected) in [
            (Failure::before("future_unsupported"), Status::Failed),
            (
                Failure::unsupported("new_capability_refused"),
                Status::Unsupported,
            ),
            (Failure::expired("custom_deadline"), Status::Expired),
            (Failure::cancelled("custom_cancellation"), Status::Cancelled),
        ] {
            assert_eq!(failure.status(), expected);
            let uncertain = failure.with_os_error(Some(13)).after_dispatch();
            assert_eq!(uncertain.status(), Status::OutcomeUnknown);
            assert_eq!(uncertain.code, failure.code);
            assert_eq!(uncertain.kind, failure.kind);
            assert_eq!(uncertain.os_error, Some(13));
        }
    }

    #[test]
    fn reserved_before_codes_remain_compatible() {
        for (code, expected) in [
            ("unsupported", Status::Unsupported),
            ("expired", Status::Expired),
            ("cancelled", Status::Cancelled),
        ] {
            assert_eq!(Failure::before(code).status(), expected);
            assert_eq!(Failure::uncertain(code).status(), Status::OutcomeUnknown);
        }
    }
}
