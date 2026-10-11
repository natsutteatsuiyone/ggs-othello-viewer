use std::time::Duration;

/// A GGS clock setting `initial/increment/extension`, in whole seconds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ClockSetting {
    pub initial: u32,
    pub increment: u32,
    pub extension: u32,
}

impl Default for ClockSetting {
    fn default() -> Self {
        ClockSetting { initial: 900, increment: 0, extension: 120 }
    }
}

impl ClockSetting {
    /// An empty field keeps its default rather than meaning zero, so `05:00//` has a 2:00 extension.
    pub fn parse(s: &str) -> Result<ClockSetting, String> {
        let mut out = ClockSetting::default();
        let fields = [&mut out.initial, &mut out.increment, &mut out.extension];
        let mut parts = s.split('/');
        for (field, part) in fields.into_iter().zip(parts.by_ref()) {
            let hms = part.split(',').next().unwrap_or("").trim();
            if !hms.is_empty() {
                *field = parse_hms(hms).ok_or_else(|| format!("bad clock field '{part}' in '{s}'"))?;
            }
        }
        if parts.next().is_some() {
            return Err(format!("too many clock fields in '{s}'"));
        }
        Ok(out)
    }
}

/// `5:00/0:00/2:00`; every field is written, since an empty one would read back as the default.
impl std::fmt::Display for ClockSetting {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let (i, n, e) = (self.initial.into(), self.increment.into(), self.extension.into());
        write!(f, "{}/{}/{}", hms(i), hms(n), hms(e))
    }
}

fn parse_hms(s: &str) -> Option<u32> {
    let mut parts = s.split(':');
    let secs =
        parts.by_ref().take(3).try_fold(0u32, |total, part| total.checked_mul(60)?.checked_add(part.parse().ok()?))?;
    if parts.next().is_some() {
        return None;
    }
    (secs <= 86400).then_some(secs)
}

/// One player's clock, reconstructed from the setting and the time field of each move.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Clock {
    remaining_us: i64,
    increment_us: i64,
    extension_us: i64,
}

impl Clock {
    pub fn new(setting: ClockSetting) -> Clock {
        let us = |s: u32| i64::from(s) * 1_000_000;
        Clock {
            remaining_us: us(setting.initial),
            increment_us: us(setting.increment),
            extension_us: us(setting.extension),
        }
    }

    /// Same order as the server's `GAME_Clock::update`: the extension rule sees the time before the increment.
    /// An invalid or overflowing charge leaves the clock unchanged.
    pub fn charge(&mut self, seconds: f64) -> Result<(), String> {
        let used_us = seconds * 1e6;
        if !seconds.is_finite() || seconds < 0.0 || used_us >= i64::MAX as f64 {
            return Err(format!("invalid elapsed time '{seconds}'"));
        }
        let mut remaining_us = self.remaining_us.checked_sub(used_us as i64).ok_or("clock underflow")?;
        let mut extension_us = self.extension_us;
        if remaining_us < 0 {
            remaining_us = remaining_us.checked_add(extension_us).ok_or("clock overflow")?;
            extension_us = 0;
        }
        remaining_us = remaining_us.checked_add(self.increment_us).ok_or("clock overflow")?;
        self.remaining_us = remaining_us;
        self.extension_us = extension_us;
        Ok(())
    }

    /// Seconds to display. A running clock counts down from its value at the last move;
    /// the stored value is truncated toward zero first. A running clock that passes zero goes into
    /// its extension, as the server's timeout event does.
    pub fn display_secs(&self, running_for: Option<Duration>) -> i64 {
        let elapsed = running_for.map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        let left = (self.remaining_us / 1_000_000).saturating_sub(elapsed);
        if left < 0 && running_for.is_some() { left + self.extension_us / 1_000_000 } else { left }
    }
}

/// `m:ss`, or `h:mm:ss` from an hour up.
pub fn hms(s: u64) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn format_secs(secs: i64) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.unsigned_abs();
    format!("{sign}{}:{:02}:{:02}", a / 3600, a / 60 % 60, a % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(i: u32, n: u32, e: u32) -> ClockSetting {
        ClockSetting { initial: i, increment: n, extension: e }
    }

    #[test]
    fn empty_fields_keep_defaults() {
        assert_eq!(ClockSetting::parse("05:00//02:00").unwrap(), setting(300, 0, 120));
        assert_eq!(ClockSetting::parse("05:00//").unwrap(), setting(300, 0, 120));
        assert_eq!(ClockSetting::parse("").unwrap(), setting(900, 0, 120));
        assert_eq!(ClockSetting::parse("1:00:00/10/0").unwrap(), setting(3600, 10, 0));
        assert_eq!(ClockSetting::parse("2:00,N/5,x").unwrap(), setting(120, 5, 120));
    }

    #[test]
    fn setting_text_round_trips() {
        for s in [setting(300, 0, 120), setting(3725, 5, 0), setting(0, 0, 0)] {
            assert_eq!(ClockSetting::parse(&s.to_string()).unwrap(), s, "{s}");
        }
        assert_eq!(setting(3725, 5, 0).to_string(), "1:02:05/0:05/0:00");
    }

    #[test]
    fn malformed_or_overflowing_fields_are_rejected() {
        assert!(ClockSetting::parse("5:00.5").is_err(), "fractional seconds");
        assert!(ClockSetting::parse("25:00:00").is_err(), "hours past 24");
        assert!(ClockSetting::parse("a:b").is_err(), "not a number");
        for s in ["71582788:16", "4294967295:0:0", "0:0:0:0", "1/2/3/4"] {
            assert!(ClockSetting::parse(s).is_err(), "{s}");
        }
    }

    #[test]
    fn invalid_charges_leave_the_clock_unchanged() {
        let mut c = Clock::new(setting(300, 2, 120));
        for seconds in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, 1e13] {
            let before = c;
            assert!(c.charge(seconds).is_err(), "{seconds}");
            assert_eq!(c, before);
        }
        for remaining_us in [i64::MIN, i64::MAX] {
            let mut c = Clock { remaining_us, increment_us: 1, extension_us: 0 };
            let before = c;
            assert!(c.charge(if remaining_us < 0 { 1.0 } else { 0.0 }).is_err());
            assert_eq!(c, before, "arithmetic failure must not partially charge the clock");
        }
        assert!(Clock::new(setting(300, 0, 0)).display_secs(Some(Duration::MAX)) < 0);
    }

    #[test]
    fn charge_applies_increment_then_extension_once() {
        let mut c = Clock::new(setting(300, 2, 120));
        c.charge(3.1).unwrap();
        assert_eq!(c.display_secs(None), 298, "300 + 2 - 3.1 truncates to 298");
        c.charge(310.0).unwrap();
        assert_eq!(c.display_secs(None), 110, "-9.1 plus the 120 s extension is 110.9");
        c.charge(200.0).unwrap();
        assert!(c.display_secs(None) < 0, "extension is used only once");
    }

    #[test]
    fn extension_is_checked_before_the_increment_is_added() {
        let mut c = Clock::new(setting(10, 5, 60));
        c.charge(12.0).unwrap();
        assert_eq!(c.display_secs(None), 63, "GAME_Clock::update: 10 - 12 < 0 takes the extension, then +5");
    }

    #[test]
    fn running_clock_counts_down_and_truncates_toward_zero() {
        let mut c = Clock::new(setting(10, 0, 0));
        assert_eq!(c.display_secs(Some(Duration::from_millis(3900))), 7);
        c.charge(10.5).unwrap();
        assert_eq!(c.display_secs(None), 0, "-0.5 s truncates to 0, not -1");
        assert_eq!(c.display_secs(Some(Duration::from_secs(3))), -3);
        let c = Clock::new(setting(10, 0, 60));
        assert_eq!(c.display_secs(Some(Duration::from_secs(12))), 58, "running past zero enters the extension");
    }

    #[test]
    fn clock_text_is_signed_hours_minutes_and_seconds() {
        assert_eq!(format_secs(297), "+0:04:57");
        assert_eq!(format_secs(-3), "-0:00:03");
        assert_eq!(format_secs(3600 + 61), "+1:01:01");
    }
}
