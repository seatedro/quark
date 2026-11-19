//! [`MotionAudit`]: a [`UiTestHarness`] stepped at 120 Hz on its fake
//! clock, one value read from every frame, and the checks a smooth motion
//! passes.

use quark::scene::Scene;

use super::UiTestHarness;
use crate::UiApp;

/// Gaps between the frames of a 120 Hz display on a whole-millisecond
/// clock: three frames every 25 ms.
const GAPS_120_HZ: [u64; 3] = [8, 8, 9];

/// One value per frame of a motion, read from the frame's scene by a probe
/// (an exiting card's opacity, a panel's x), with `None` for a frame the
/// probe found nothing in.
///
/// ```ignore
/// ui.app_mut().show_card = false;
/// let audit = MotionAudit::until_settled(&mut ui, 2_000, card_opacity);
/// audit.assert_monotonic(false);
/// audit.assert_no_snaps(0.06);
/// audit.assert_no_vanishing(0.0, 0.02);
/// ```
#[derive(Debug, Clone, Default)]
pub struct MotionAudit {
    /// `(clock ms, value)` of each frame, in order.
    samples: Vec<(u64, Option<f32>)>,
}

impl MotionAudit {
    /// Draw the main window now and then every 120 Hz frame until no frame
    /// is pending or `limit_ms` passed, probing each.
    ///
    /// # Panics
    ///
    /// When frames are still pending after `limit_ms`.
    pub fn until_settled<U: UiApp>(
        ui: &mut UiTestHarness<U>,
        limit_ms: u64,
        probe: impl Fn(&Scene) -> Option<f32>,
    ) -> Self {
        let mut audit = Self::default();
        let start = ui.now();
        audit.sample(ui, &probe);
        for gap in GAPS_120_HZ.iter().cycle() {
            if ui.next_frame_in().is_none() {
                return audit;
            }
            let elapsed = (ui.now() - start).as_millis() as u64;
            assert!(
                elapsed < limit_ms,
                "still moving after {limit_ms} ms: {:?}",
                audit.samples
            );
            ui.advance(*gap);
            audit.sample(ui, &probe);
        }
        unreachable!("the gaps cycle forever")
    }

    /// Draw the main window now and every 120 Hz frame for `duration_ms`,
    /// probing each, whether or not anything moves.
    pub fn for_ms<U: UiApp>(
        ui: &mut UiTestHarness<U>,
        duration_ms: u64,
        probe: impl Fn(&Scene) -> Option<f32>,
    ) -> Self {
        let mut audit = Self::default();
        let end = ui.now().as_millis() as u64 + duration_ms;
        audit.sample(ui, &probe);
        for gap in GAPS_120_HZ.iter().cycle() {
            if ui.now().as_millis() as u64 + gap > end {
                return audit;
            }
            ui.advance(*gap);
            audit.sample(ui, &probe);
        }
        unreachable!("the gaps cycle forever")
    }

    fn sample<U: UiApp>(
        &mut self,
        ui: &mut UiTestHarness<U>,
        probe: impl Fn(&Scene) -> Option<f32>,
    ) {
        let value = probe(ui.frame());
        self.samples.push((ui.now().as_millis() as u64, value));
    }

    /// `(clock ms, value)` of each frame, in order.
    pub fn samples(&self) -> &[(u64, Option<f32>)] {
        &self.samples
    }

    /// The value of the last frame.
    pub fn last(&self) -> Option<f32> {
        self.samples.last().and_then(|(_, value)| *value)
    }

    /// Append `other`'s frames, for a motion audited in parts.
    pub fn then(mut self, other: Self) -> Self {
        self.samples.extend(other.samples);
        self
    }

    /// Panics when the value moves more than `max_step` between two frames
    /// it was found in: a snap.
    pub fn assert_no_snaps(&self, max_step: f32) {
        let mut found = self.samples.iter().filter_map(|&(ms, v)| Some((ms, v?)));
        let Some(mut before) = found.next() else {
            return;
        };
        for now in found {
            assert!(
                (now.1 - before.1).abs() <= max_step,
                "snapped from {} at {} ms to {} at {} ms: {:?}",
                before.1,
                before.0,
                now.1,
                now.0,
                self.samples
            );
            before = now;
        }
    }

    /// Panics when the value ever moves against the direction: up when
    /// `rising`, down otherwise.
    pub fn assert_monotonic(&self, rising: bool) {
        let found: Vec<(u64, f32)> = self
            .samples
            .iter()
            .filter_map(|&(ms, v)| Some((ms, v?)))
            .collect();
        for pair in found.windows(2) {
            let (a, b) = (pair[0].1, pair[1].1);
            let backwards = if rising { b < a } else { b > a };
            assert!(
                !backwards,
                "turned back from {a} to {b} at {} ms: {:?}",
                pair[1].0, self.samples
            );
        }
    }

    /// Panics when the probe stops finding what it reads while the value
    /// is still more than `epsilon` from `rest`: a node that disappeared
    /// before its exit settled, or a frame that skipped it.
    pub fn assert_no_vanishing(&self, rest: f32, epsilon: f32) {
        for pair in self.samples.windows(2) {
            if let ((_, Some(before)), (ms, None)) = (pair[0], pair[1]) {
                assert!(
                    (before - rest).abs() <= epsilon,
                    "vanished at {ms} ms while at {before}: {:?}",
                    self.samples
                );
            }
        }
    }
}
