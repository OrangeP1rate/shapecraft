//! Scrolling that feels like a phone's: the page follows the finger, keeps going after a flick and slows to a stop
//! (UIKit's normal deceleration, 0.998 of the speed left per millisecond), stretches with resistance past either end
//! while dragged, and springs back when let go there.

/// How much speed is left after a second: UIScrollView's normal rate, 0.998 per millisecond.
const DECEL_PER_S: f32 = 0.135_065_6; // 0.998^1000
/// The spring that pulls an overscrolled page back (stiffness, per second squared), critically damped.
const SPRING: f32 = 220.0;
/// Below this (points a second) a glide is over.
const REST: f32 = 6.0;
/// The finger's samples kept for its speed when it lets go (seconds).
const WINDOW: f64 = 0.08;

/// One axis of a scrolling page: its offset (0 at the start, `max` at the end) and how it moves.
#[derive(Clone, Debug, Default)]
pub struct Glide {
    /// How far it's scrolled; outside 0..=max while stretched past an end.
    pub offset: f32,
    vel: f32,
    /// The finger's recent (time, offset) samples while it's down.
    samples: Vec<(f64, f32)>,
    held: bool,
}

impl Glide {
    /// Whether it's still moving on its own (the caller keeps asking for frames).
    #[cfg(test)]
    pub fn moving(&self) -> bool {
        !self.held && self.vel != 0.0
    }

    /// Stops it where it is.
    pub fn stop(&mut self) {
        self.vel = 0.0;
        self.samples.clear();
    }

    /// Jumps to an offset and stops (a new page).
    pub fn reset(&mut self, offset: f32) {
        self.offset = offset;
        self.stop();
    }

    /// The finger moved the page by `delta` points (positive scrolls on, as a drag up or left does).
    pub fn drag(&mut self, delta: f32, max: f32, extent: f32, now: f64) {
        self.held = true;
        self.vel = 0.0;
        // Past an end the page moves less and less for the same finger travel, as UIKit's does.
        let over = self.over(max);
        let d = if over != 0.0 && over.signum() == delta.signum() {
            let k = 1.0 / (1.0 + over.abs() * 0.55 / extent.max(1.0));
            delta * k * k
        } else {
            delta
        };
        self.offset += d;
        self.samples.push((now, self.offset));
        self.samples.retain(|(t, _)| now - t <= WINDOW);
    }

    /// The finger let go: the page keeps the speed it had.
    pub fn release(&mut self, now: f64) {
        self.held = false;
        self.samples.retain(|(t, _)| now - t <= WINDOW);
        self.vel = match (self.samples.first(), self.samples.last()) {
            (Some(&(t0, a)), Some(&(t1, b))) if t1 - t0 > 0.005 => (b - a) / (t1 - t0) as f32,
            _ => 0.0,
        };
        // A flick never throws faster than this (points a second).
        self.vel = self.vel.clamp(-8000.0, 8000.0);
        self.samples.clear();
        if self.vel.abs() < REST {
            // (Let go stretched past an end, `step` springs it back.)
            self.vel = 0.0;
        }
    }

    /// Wheel or trackpad scrolling: moves at once, no glide (the system's own events already glide).
    pub fn nudge(&mut self, delta: f32, max: f32) {
        self.offset = (self.offset + delta).clamp(0.0, max.max(0.0));
        self.vel = 0.0;
    }

    /// One frame of `dt` seconds with nothing holding the page. Returns whether it's still moving.
    pub fn step(&mut self, dt: f32, max: f32) -> bool {
        let max = max.max(0.0);
        if self.held {
            return false;
        }
        if self.vel == 0.0 {
            // Content shrank under a page left past its end: settle without a glide.
            if self.offset < 0.0 || self.offset > max {
                self.vel = f32::MIN_POSITIVE;
            } else {
                return false;
            }
        }
        let dt = dt.clamp(0.0, 0.05);
        // A few small steps keep the spring steady at low frame rates.
        let n = (dt / 0.004).ceil().max(1.0) as usize;
        let h = dt / n as f32;
        for _ in 0..n {
            let over = self.over(max);
            if over != 0.0 {
                // Past an end: a critically damped spring back to it (and a fast glide into it stops quickly).
                let a = -SPRING * over - 2.0 * SPRING.sqrt() * self.vel;
                self.vel += a * h;
                self.offset += self.vel * h;
                let back = self.over(max);
                // Home (or across, from a step too big): rest at the end.
                if back == 0.0 || back.signum() != over.signum() || back.abs() < 0.3 && self.vel.abs() < 20.0 {
                    self.offset = self.offset.clamp(0.0, max);
                    self.vel = 0.0;
                    return false;
                }
            } else {
                self.vel *= DECEL_PER_S.powf(h);
                self.offset += self.vel * h;
                if self.vel.abs() < REST && self.over(max) == 0.0 {
                    self.vel = 0.0;
                    return false;
                }
            }
        }
        true
    }

    /// How far past an end it is: negative before the start, positive after the end.
    fn over(&self, max: f32) -> f32 {
        if self.offset < 0.0 {
            self.offset
        } else if self.offset > max {
            self.offset - max
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flick(g: &mut Glide, speed: f32) {
        // A finger moving `speed` points a second for a tenth of a second, at 120 frames a second.
        for i in 0..12 {
            g.drag(speed / 120.0, 10_000.0, 400.0, i as f64 / 120.0);
        }
        g.release(12.0 / 120.0);
    }

    #[test]
    fn a_flick_glides_on_and_slows_to_a_stop() {
        let mut g = Glide::default();
        flick(&mut g, 2000.0);
        let let_go = g.offset;
        assert!(g.moving());
        let mut t = 0.0;
        while g.step(1.0 / 120.0, 10_000.0) {
            t += 1.0 / 120.0;
            assert!(t < 10.0, "never stopped");
        }
        // UIKit's rate: v / ln(1/0.998^1000) ≈ 2000 / 2.0 ≈ 1000 points further, over a couple of seconds.
        let went = g.offset - let_go;
        assert!((900.0..1100.0).contains(&went), "went {went}");
        assert!((1.5..4.5).contains(&t), "took {t}");
    }

    #[test]
    fn a_finger_that_stops_before_letting_go_throws_nothing() {
        let mut g = Glide::default();
        flick(&mut g, 2000.0);
        g.stop();
        for i in 0..20 {
            g.drag(0.0, 10_000.0, 400.0, 1.0 + i as f64 / 120.0);
        }
        g.release(1.0 + 20.0 / 120.0);
        assert!(!g.moving());
    }

    #[test]
    fn a_glide_into_the_end_springs_back_to_it() {
        let mut g = Glide::default();
        flick(&mut g, 3000.0);
        let mut peak: f32 = 0.0;
        let mut t = 0.0;
        while g.step(1.0 / 60.0, 300.0) {
            peak = peak.max(g.offset);
            t += 1.0 / 60.0;
            assert!(t < 5.0, "never settled");
        }
        assert!(peak > 300.0, "no bounce past the end");
        assert!(peak < 450.0, "bounced too far: {peak}");
        assert!((g.offset - 300.0).abs() < 0.5);
    }

    #[test]
    fn dragging_past_the_start_stretches_and_lets_go_back() {
        let mut g = Glide::default();
        for i in 0..30 {
            g.drag(-10.0, 1000.0, 400.0, i as f64 / 60.0);
        }
        assert!(g.offset < 0.0 && g.offset > -300.0, "stretch {}", g.offset);
        // Held still, then let go.
        for i in 0..10 {
            g.drag(0.0, 1000.0, 400.0, 1.0 + i as f64 / 60.0);
        }
        g.release(1.2);
        let mut t = 0.0;
        while g.step(1.0 / 60.0, 1000.0) {
            t += 1.0 / 60.0;
            assert!(t < 3.0);
        }
        assert_eq!(g.offset, 0.0);
    }
}
