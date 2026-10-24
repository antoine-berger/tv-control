//! Resolve relative/native-percentage volume once, under device exclusion.
use tv_domain::{generic::Intent, Failure, Observed, PortResult};

pub(super) struct Target {
    pub index: u16,
    pub max_index: u16,
}
impl Target {
    pub fn resolve(intent: &Intent, before: &Observed) -> PortResult<Option<Self>> {
        if !matches!(
            intent,
            Intent::AdjustVolume { .. } | Intent::SetVolumePercent { .. }
        ) {
            return Ok(None);
        }
        let max_index = before
            .volume_max_index
            .filter(|max| (1..=1000).contains(max))
            .ok_or(Failure::unsupported("volume_scale_unavailable"))?;
        let current = before
            .volume_index
            .filter(|index| *index <= max_index)
            .ok_or(Failure::unsupported("volume_state_unavailable"))?;
        let index = match intent {
            Intent::AdjustVolume { delta } => {
                let target = i32::from(current) + i32::from(*delta);
                if !(0..=i32::from(max_index)).contains(&target) {
                    return Err(Failure::before("volume_out_of_range"));
                }
                target as u16
            }
            Intent::SetVolumePercent { percent } => {
                // Integer rounding half up. This is a fraction of the native index
                // scale, not a promise of proportional acoustic loudness.
                ((u32::from(max_index) * u32::from(*percent) + 50) / 100) as u16
            }
            _ => unreachable!(),
        };
        Ok(Some(Self { index, max_index }))
    }
    pub fn check_scale(&self, state: &Observed) -> PortResult<()> {
        if state.volume_max_index != Some(self.max_index) {
            return Err(Failure::before("volume_scale_changed"));
        }
        Ok(())
    }
}
