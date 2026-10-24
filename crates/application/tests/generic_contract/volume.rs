use super::*;

#[derive(Default)]
struct VolumeDevice {
    state: Mutex<Observed>,
    targets: Mutex<Vec<(u16, u16)>>,
    reads: AtomicUsize,
    fail_dispatch: AtomicBool,
    change_scale_after_dispatch: AtomicBool,
    delay_confirmation: AtomicBool,
    cancel_on_read: AtomicBool,
    cancel_on_dispatch: AtomicBool,
    expire_on_read: AtomicBool,
    cancel_flag: Arc<AtomicBool>,
    journal: Arc<Memory>,
}
struct SharedVolume(Arc<VolumeDevice>);
impl Device for SharedVolume {
    fn resource(&self, _: &str) -> PortResult<String> {
        Ok("volume_fixture".into())
    }
    fn authorized(&self, _: &str) -> PortResult<()> {
        Ok(())
    }
    fn apps(&self, _: &str, _: &Context<'_>) -> PortResult<Vec<App>> {
        Ok(vec![])
    }
    fn observe(&self, _: &str, ctx: &Context<'_>) -> PortResult<Observed> {
        // Fresh calculation is only allowed after reserving the original intent.
        assert!(!self.0.journal.entries.lock().unwrap().is_empty());
        let reads = self.0.reads.fetch_add(1, Ordering::SeqCst);
        if self.0.cancel_on_read.load(Ordering::SeqCst) {
            self.0.cancel_flag.store(true, Ordering::SeqCst);
        }
        if self.0.expire_on_read.load(Ordering::SeqCst) {
            ctx.clock.sleep_ms(1000);
        }
        let mut state = self.0.state.lock().unwrap().clone();
        if reads > 0 && self.0.change_scale_after_dispatch.load(Ordering::SeqCst) {
            state.volume_max_index = state.volume_max_index.map(|v| v + 1);
        }
        if reads == 1 && self.0.delay_confirmation.load(Ordering::SeqCst) {
            state.volume_index = state.volume_index.map(|v| v.saturating_sub(1));
        }
        Ok(state)
    }
    fn ui(&self, _: &str, _: &Context<'_>) -> PortResult<UiSnapshot> {
        unreachable!("volume never needs UI")
    }
    fn apply(&self, _: &str, _: &Intent, _: Option<&App>, _: &Context<'_>) -> PortResult<()> {
        unreachable!("relative volume uses the guarded absolute-target port")
    }
    fn set_volume_target(
        &self,
        _: &str,
        index: u16,
        expected_max: u16,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        ctx.check()?;
        self.0.targets.lock().unwrap().push((index, expected_max));
        if self.0.fail_dispatch.load(Ordering::SeqCst) {
            return Err(Failure::uncertain("transport_lost"));
        }
        self.0.state.lock().unwrap().volume_index = Some(index);
        if self.0.cancel_on_dispatch.load(Ordering::SeqCst) {
            self.0.cancel_flag.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}
fn setup_volume(
    index: Option<u16>,
    max: Option<u16>,
) -> (generic::Service<SharedVolume, Time>, Arc<VolumeDevice>) {
    let d = Arc::new(VolumeDevice {
        state: Mutex::new(Observed {
            connected: true,
            volume_index: index,
            volume_max_index: max,
            ..Observed::default()
        }),
        ..VolumeDevice::default()
    });
    (
        generic::Service::new(
            SharedVolume(d.clone()),
            d.journal.clone(),
            Time::default(),
            10000,
            10,
        ),
        d,
    )
}
#[test]
fn relative_volume_uses_fresh_native_index_once_and_caches_original_intent() {
    let (s, d) = setup_volume(Some(8), Some(15));
    d.delay_confirmation.store(true, Ordering::SeqCst);
    let request = req(Intent::AdjustVolume { delta: -3 });
    let result = s.execute(request.clone(), &Never);
    assert_eq!(result.status, Status::Verified);
    assert_eq!(result.observed.unwrap().volume_index, Some(5));
    assert_eq!(*d.targets.lock().unwrap(), vec![(5, 15)]);
    assert_eq!(d.reads.load(Ordering::SeqCst), 3);
    d.state.lock().unwrap().volume_index = Some(12);
    assert!(s.execute(request, &Never).cached);
    assert_eq!(d.reads.load(Ordering::SeqCst), 3);
    assert_eq!(d.targets.lock().unwrap().len(), 1);
    assert_eq!(
        s.execute(req(Intent::AdjustVolume { delta: 1 }), &Never)
            .code,
        "request_id_conflict"
    );
}
#[test]
fn percentages_use_native_scale_round_half_up_and_exact_endpoints() {
    for (max, percent, target) in [
        (15, 50, 8),
        (7, 50, 4),
        (31, 33, 10),
        (15, 0, 0),
        (15, 100, 15),
        (1000, 99, 990),
    ] {
        let (s, d) = setup_volume(Some(3), Some(max));
        let result = s.execute(req(Intent::SetVolumePercent { percent }), &Never);
        assert_eq!(result.status, Status::Verified, "{max}/{percent}");
        assert_eq!(result.observed.unwrap().volume_index, Some(target));
        assert_eq!(*d.targets.lock().unwrap(), vec![(target, max)]);
    }
}
#[test]
fn zero_delta_and_already_matching_percentage_do_not_dispatch() {
    for intent in [
        Intent::AdjustVolume { delta: 0 },
        Intent::SetVolumePercent { percent: 50 },
    ] {
        let (s, d) = setup_volume(Some(8), Some(15));
        assert_eq!(s.execute(req(intent), &Never).status, Status::Verified);
        assert!(d.targets.lock().unwrap().is_empty());
    }
}
#[test]
fn missing_or_invalid_native_scale_and_index_refuse_before_dispatch() {
    for (index, max, code) in [
        (Some(1), None, "volume_scale_unavailable"),
        (Some(0), Some(0), "volume_scale_unavailable"),
        (Some(0), Some(1001), "volume_scale_unavailable"),
        (None, Some(15), "volume_state_unavailable"),
        (Some(16), Some(15), "volume_state_unavailable"),
    ] {
        for intent in [
            Intent::AdjustVolume { delta: 1 },
            Intent::SetVolumePercent { percent: 50 },
        ] {
            let (s, d) = setup_volume(index, max);
            let result = s.execute(req(intent), &Never);
            assert_eq!(result.code, code);
            assert_eq!(result.status, Status::Unsupported);
            assert!(d.targets.lock().unwrap().is_empty());
        }
    }
}
#[test]
fn out_of_range_relative_targets_never_saturate_or_dispatch() {
    for (index, delta) in [(0, -1), (15, 1), (8, 1000), (8, -1000)] {
        let (s, d) = setup_volume(Some(index), Some(15));
        assert_eq!(
            s.execute(req(Intent::AdjustVolume { delta }), &Never).code,
            "volume_out_of_range"
        );
        assert!(d.targets.lock().unwrap().is_empty());
    }
}
#[test]
fn invalid_percent_and_delta_are_rejected_before_read_or_reservation() {
    for intent in [
        Intent::SetVolumePercent { percent: 101 },
        Intent::SetVolumePercent { percent: 255 },
        Intent::AdjustVolume { delta: i16::MIN },
        Intent::AdjustVolume { delta: i16::MAX },
    ] {
        let (s, d) = setup_volume(Some(8), Some(15));
        assert_eq!(s.execute(req(intent), &Never).status, Status::Invalid);
        assert_eq!(d.reads.load(Ordering::SeqCst), 0);
        assert!(d.journal.entries.lock().unwrap().is_empty());
    }
}
#[test]
fn failed_or_uncertain_mutations_never_replay_a_relative_adjustment() {
    let (s, d) = setup_volume(Some(8), Some(15));
    d.fail_dispatch.store(true, Ordering::SeqCst);
    let request = req(Intent::AdjustVolume { delta: 1 });
    assert_eq!(
        s.execute(request.clone(), &Never).status,
        Status::OutcomeUnknown
    );
    let replay = s.execute(request, &Never);
    assert!(replay.cached);
    assert_eq!(replay.status, Status::OutcomeUnknown);
    assert_eq!(d.targets.lock().unwrap().len(), 1);
}
#[test]
fn changed_scale_after_dispatch_is_uncertain_and_never_repeated() {
    let (s, d) = setup_volume(Some(8), Some(15));
    d.change_scale_after_dispatch.store(true, Ordering::SeqCst);
    let result = s.execute(req(Intent::SetVolumePercent { percent: 20 }), &Never);
    assert_eq!(result.status, Status::OutcomeUnknown);
    assert_eq!(result.code, "volume_scale_changed");
    assert_eq!(d.targets.lock().unwrap().len(), 1);
}
struct CancelOn(Arc<AtomicBool>);
impl Cancellation for CancelOn {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
#[test]
fn cancellation_after_fresh_read_and_expiry_before_read_do_not_dispatch() {
    let (s, d) = setup_volume(Some(8), Some(15));
    d.cancel_on_read.store(true, Ordering::SeqCst);
    let cancel = CancelOn(d.cancel_flag.clone());
    assert_eq!(
        s.execute(req(Intent::AdjustVolume { delta: 1 }), &cancel)
            .status,
        Status::Cancelled
    );
    assert!(d.targets.lock().unwrap().is_empty());
    let (s, d) = setup_volume(Some(8), Some(15));
    let mut request = req(Intent::AdjustVolume { delta: 1 });
    request.expires_at_unix_ms = 1000;
    assert_eq!(s.execute(request, &Never).status, Status::Expired);
    assert_eq!(d.reads.load(Ordering::SeqCst), 0);
    assert!(d.targets.lock().unwrap().is_empty());
}
#[test]
fn backends_without_guarded_absolute_volume_report_unsupported() {
    let (s, d, _) = setup(vec![]);
    {
        let mut state = d.state.lock().unwrap();
        state.volume_index = Some(8);
        state.volume_max_index = Some(15);
    }
    assert_eq!(
        s.execute(req(Intent::AdjustVolume { delta: 1 }), &Never)
            .status,
        Status::Unsupported
    );
    assert_eq!(d.effects.load(Ordering::SeqCst), 0);
}

#[test]
fn no_op_still_requires_live_deadline_and_cancellation_after_last_read() {
    for intent in [
        Intent::AdjustVolume { delta: 0 },
        Intent::SetVolumePercent { percent: 50 },
    ] {
        let (s, d) = setup_volume(Some(8), Some(15));
        d.expire_on_read.store(true, Ordering::SeqCst);
        assert_eq!(
            s.execute(req(intent.clone()), &Never).status,
            Status::Expired
        );
        assert!(d.targets.lock().unwrap().is_empty());
        let (s, d) = setup_volume(Some(8), Some(15));
        d.cancel_on_read.store(true, Ordering::SeqCst);
        let cancel = CancelOn(d.cancel_flag.clone());
        assert_eq!(s.execute(req(intent), &cancel).status, Status::Cancelled);
        assert!(d.targets.lock().unwrap().is_empty());
    }
}
#[test]
fn cancellation_after_volume_dispatch_is_uncertain_without_replay() {
    let (s, d) = setup_volume(Some(8), Some(15));
    d.cancel_on_dispatch.store(true, Ordering::SeqCst);
    let cancel = CancelOn(d.cancel_flag.clone());
    let request = req(Intent::AdjustVolume { delta: 1 });
    let result = s.execute(request.clone(), &cancel);
    assert_eq!(result.status, Status::OutcomeUnknown);
    d.cancel_flag.store(false, Ordering::SeqCst);
    assert!(s.execute(request, &cancel).cached);
    assert_eq!(d.targets.lock().unwrap().len(), 1);
}
