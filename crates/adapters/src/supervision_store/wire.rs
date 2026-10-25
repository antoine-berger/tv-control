use super::{fail, Binding, MAX_BYTES};
use serde::{Deserialize, Serialize};
use tv_application::supervision::{Code, Mode, Phase, Record, Registration};
use tv_domain::PortResult;

macro_rules! wire_enum {
    ($wire:ident, $domain:ident, [$($variant:ident),+ $(,)?]) => {
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum $wire { $($variant),+ }
        impl From<$domain> for $wire {
            fn from(value: $domain) -> Self { match value { $($domain::$variant => Self::$variant),+ } }
        }
        impl From<$wire> for $domain {
            fn from(value: $wire) -> Self { match value { $($wire::$variant => Self::$variant),+ } }
        }
    }
}
wire_enum!(
    RegistrationWire,
    Registration,
    [Prepared, Installed, Unknown, Removed]
);
wire_enum!(ModeWire, Mode, [Stopped, Enabled, Blocked]);
wire_enum!(
    PhaseWire,
    Phase,
    [Idle, Monitoring, StartInFlight, CleanupInFlight]
);
wire_enum!(
    CodeWire,
    Code,
    [
        Prepared,
        Stopped,
        Armed,
        Monitoring,
        Ready,
        Degraded,
        Starting,
        StartUncertain,
        BackendRefused,
        StartBudgetExhausted,
        UncleanBudgetExhausted,
        InterruptedLifecycle,
        CleanupPending,
        CleanupUnverified,
        CleanupVerified,
    ]
);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordWire {
    revision: u64,
    arm: u64,
    registration: RegistrationWire,
    mode: ModeWire,
    phase: PhaseWire,
    starts: u8,
    unclean: u8,
    code: CodeWire,
}
impl From<&Record> for RecordWire {
    fn from(value: &Record) -> Self {
        Self {
            revision: value.revision,
            arm: value.arm,
            registration: value.registration.into(),
            mode: value.mode.into(),
            phase: value.phase.into(),
            starts: value.starts,
            unclean: value.unclean,
            code: value.code.into(),
        }
    }
}
impl From<RecordWire> for Record {
    fn from(value: RecordWire) -> Self {
        Self {
            revision: value.revision,
            arm: value.arm,
            registration: value.registration.into(),
            mode: value.mode.into(),
            phase: value.phase.into(),
            starts: value.starts,
            unclean: value.unclean,
            code: value.code.into(),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Disk {
    schema: u32,
    binding: Binding,
    record: RecordWire,
}
pub(super) fn encode(binding: &Binding, record: &Record) -> PortResult<Vec<u8>> {
    binding.validate()?;
    record.validate()?;
    let bytes = serde_json::to_vec(&Disk {
        schema: 1,
        binding: binding.clone(),
        record: record.into(),
    })
    .map_err(|_| fail("supervision_store_encode_failed"))?;
    if bytes.len() > MAX_BYTES {
        return Err(fail("supervision_store_limit"));
    }
    Ok(bytes)
}
pub(super) fn decode(bytes: &[u8]) -> PortResult<(Binding, Record)> {
    if bytes.len() > MAX_BYTES {
        return Err(fail("supervision_store_limit"));
    }
    let value: Disk =
        serde_json::from_slice(bytes).map_err(|_| fail("supervision_store_corrupt"))?;
    if value.schema != 1 {
        return Err(fail("supervision_store_corrupt"));
    }
    value
        .binding
        .validate()
        .map_err(|_| fail("supervision_store_corrupt"))?;
    let record: Record = value.record.into();
    record
        .validate()
        .map_err(|_| fail("supervision_store_corrupt"))?;
    Ok((value.binding, record))
}
