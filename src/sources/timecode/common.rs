use serde::{Deserialize, Serialize};

/// One received timecode frame, with the rate it counts at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub fps: f64,
    pub drop_frame: bool,
}

impl Timecode {
    /// The default SMPTE label, `HH:MM:SS:FF`, with `;` for drop frame.
    pub fn label(&self) -> String {
        return format!(
            "{:02}:{:02}:{:02}{}{:02}",
            self.hours,
            self.minutes,
            self.seconds,
            if self.drop_frame { ';' } else { ':' },
            self.frames
        );
    }

    /// The timecode's nominal frame rate: 24, 25, 30 or 60.
    pub fn nominal_fps(&self) -> u64 {
        return self.fps.round() as u64;
    }

    /// Real frame rate and drop-frame flag for a timecode rate code:
    /// 0 = 24, 1 = 25, 2 = 29.97 drop-frame, 3 = 30.
    fn rate(code: u8) -> Option<(f64, bool)> {
        return match code {
            0 => Some((24.0, false)),
            1 => Some((25.0, false)),
            2 => Some((30_000.0 / 1001.0, true)),
            3 => Some((30.0, false)),
            _ => None,
        };
    }

    /// Checked timecode from label fields and a rate code.
    pub(super) fn from_rate(hours: u8, minutes: u8, seconds: u8, frames: u8, code: u8) -> Option<Timecode> {
        let (fps, drop_frame) = Self::rate(code)?;
        if hours > 23 || minutes > 59 || seconds > 59 || u64::from(frames) >= fps.round() as u64 {
            return None;
        }
        return Some(Timecode {
            hours,
            minutes,
            seconds,
            frames,
            fps,
            drop_frame,
        });
    }

    /// Move the timecode forward by `frames`, carrying seconds/minutes/hours and
    /// skipping drop-frame numbers at a minute boundary.
    pub(super) fn add_frames(&self, frames: u8) -> Timecode {
        let mut timecode = *self;
        for _ in 0..frames {
            timecode.frames += 1;
            if u64::from(timecode.frames) < timecode.nominal_fps() {
                continue;
            }
            timecode.frames = 0;
            timecode.seconds += 1;
            if timecode.seconds == 60 {
                timecode.seconds = 0;
                timecode.minutes += 1;
                if timecode.minutes == 60 {
                    timecode.minutes = 0;
                    timecode.hours = (timecode.hours + 1) % 24;
                }
            }
            if timecode.drop_frame && timecode.seconds == 0 && timecode.minutes % 10 != 0 {
                timecode.frames = (timecode.nominal_fps() * 2 / 30) as u8;
            }
        }
        return timecode;
    }

    /// Frames elapsed since 00:00:00:00, with drop-frame numbers removed.
    pub fn frame_index(&self) -> u64 {
        let fps = self.nominal_fps();
        let minutes = u64::from(self.hours) * 60 + u64::from(self.minutes);
        let raw = (u64::from(self.hours) * 3600 + u64::from(self.minutes) * 60 + u64::from(self.seconds)) * fps + u64::from(self.frames);
        // SMPTE drops two frame numbers per minute for every 30 nominal frames.
        let dropped = fps * 2 / 30;
        return if self.drop_frame {
            raw - dropped * (minutes - minutes / 10)
        } else {
            raw
        };
    }

    /// Seconds elapsed since 00:00:00:00, using the real frame rate.
    pub fn total_seconds(&self) -> f64 {
        return self.frame_index() as f64 / self.fps;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimecodeDisplay {
    #[default]
    Full,
    Milliseconds,
    NoFrames,
    Hours,
    Minutes,
    Frames,
    Seconds,
}

impl TimecodeDisplay {
    pub const ALL: [Self; 7] = [
        Self::Full,
        Self::Milliseconds,
        Self::NoFrames,
        Self::Hours,
        Self::Minutes,
        Self::Frames,
        Self::Seconds,
    ];

    pub fn label(&self) -> &'static str {
        return match self {
            Self::Full => "HH:MM:SS:FF",
            Self::Milliseconds => "HH:MM:SS.mmm",
            Self::NoFrames => "HH:MM:SS",
            Self::Hours => "HH:MM",
            Self::Minutes => "MM:SS",
            Self::Frames => "Frames",
            Self::Seconds => "Seconds",
        };
    }

    pub fn placeholder(&self) -> &'static str {
        return match self {
            Self::Full => "--:--:--:--",
            Self::Milliseconds => "--:--:--.---",
            Self::NoFrames => "--:--:--",
            Self::Hours => "--:--",
            Self::Minutes => "--:--",
            Self::Frames => "-------",
            Self::Seconds => "-----.---",
        };
    }

    pub fn format(self, timecode: &Timecode) -> String {
        return match self {
            Self::Full => timecode.label(),
            Self::NoFrames => format!("{:02}:{:02}:{:02}", timecode.hours, timecode.minutes, timecode.seconds),
            Self::Hours => format!("{:02}:{:02}", timecode.hours, timecode.minutes),
            Self::Minutes => format!("{:02}:{:02}", timecode.minutes, timecode.seconds),
            Self::Frames => timecode.frame_index().to_string(),
            Self::Seconds => format!("{:.3}", timecode.total_seconds()),
            Self::Milliseconds => {
                let millis = (timecode.frames as f64 / timecode.fps * 1000.0).round() as u32;
                format!("{:02}:{:02}:{:02}.{millis:03}", timecode.hours, timecode.minutes, timecode.seconds)
            }
        };
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimecodeStatus {
    /// The last value received, if one ever arrived.
    pub timecode: Option<Timecode>,
    pub state: TimecodeState,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum TimecodeState {
    #[default]
    Waiting,
    Receiving,
    Lost,
    Error(String),
}

#[cfg(test)]
pub(super) fn test_timecode() -> Timecode {
    return Timecode {
        hours: 1,
        minutes: 2,
        seconds: 3,
        frames: 4,
        fps: 25.0,
        drop_frame: false,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_label_is_smpte() {
        assert_eq!(test_timecode().label(), "01:02:03:04");
        assert_eq!(
            Timecode {
                drop_frame: true,
                ..test_timecode()
            }
            .label(),
            "01:02:03;04"
        );
    }

    #[test]
    fn frame_index_counts_real_frames() {
        let non_drop = test_timecode();
        assert_eq!(non_drop.frame_index(), 3723 * 25 + 4);
        assert!((non_drop.total_seconds() - 3723.16).abs() < 1e-9);

        let drop_frame = Timecode {
            hours: 0,
            minutes: 1,
            seconds: 0,
            frames: 2,
            fps: 30_000.0 / 1001.0,
            drop_frame: true,
        };
        assert_eq!(drop_frame.frame_index(), 1800, "two frame numbers were skipped");
        assert!((drop_frame.total_seconds() - 60.06).abs() < 1e-6);

        let high_rate = Timecode {
            hours: 0,
            minutes: 1,
            seconds: 0,
            frames: 4,
            fps: 60_000.0 / 1001.0,
            drop_frame: true,
        };
        assert_eq!(high_rate.frame_index(), 3600, "four frame numbers were skipped");
        assert!((high_rate.total_seconds() - 60.06).abs() < 1e-6);

        let tenth_minute = Timecode {
            minutes: 10,
            frames: 0,
            fps: 30_000.0 / 1001.0,
            drop_frame: true,
            ..drop_frame
        };
        assert_eq!(tenth_minute.frame_index(), 17_982, "the tenth minute keeps every number");
    }

    #[test]
    fn add_frames_carries_the_label() {
        assert_eq!(test_timecode().add_frames(2).label(), "01:02:03:06");

        let minute = Timecode {
            hours: 0,
            minutes: 0,
            seconds: 59,
            frames: 29,
            fps: 30.0,
            drop_frame: false,
        };
        assert_eq!(minute.add_frames(2).label(), "00:01:00:01");

        let midnight = Timecode {
            hours: 23,
            minutes: 59,
            seconds: 59,
            frames: 23,
            fps: 24.0,
            drop_frame: false,
        };
        assert_eq!(midnight.add_frames(2).label(), "00:00:00:01");
    }

    #[test]
    fn add_frames_skips_drop_frame_numbers() {
        let minute = Timecode {
            hours: 0,
            minutes: 0,
            seconds: 59,
            frames: 29,
            fps: 30_000.0 / 1001.0,
            drop_frame: true,
        };
        assert_eq!(minute.add_frames(2).label(), "00:01:00;03");

        let tenth_minute = Timecode { minutes: 9, ..minute };
        assert_eq!(tenth_minute.add_frames(2).label(), "00:10:00;01");

        let high_rate = Timecode {
            hours: 0,
            minutes: 0,
            seconds: 59,
            frames: 59,
            fps: 60_000.0 / 1001.0,
            drop_frame: true,
        };
        assert_eq!(high_rate.add_frames(2).label(), "00:01:00;05");
    }

    #[test]
    fn displays_format_the_raw_fields() {
        let timecode = test_timecode();
        assert_eq!(TimecodeDisplay::Full.format(&timecode), "01:02:03:04");
        assert_eq!(TimecodeDisplay::NoFrames.format(&timecode), "01:02:03");
        assert_eq!(TimecodeDisplay::Hours.format(&timecode), "01:02");
        assert_eq!(TimecodeDisplay::Minutes.format(&timecode), "02:03");
        assert_eq!(TimecodeDisplay::Frames.format(&timecode), "93079");
        assert_eq!(TimecodeDisplay::Seconds.format(&timecode), "3723.160");
        assert_eq!(TimecodeDisplay::Milliseconds.format(&timecode), "01:02:03.160");

        assert_eq!(TimecodeDisplay::Full.placeholder(), "--:--:--:--");
        assert_eq!(TimecodeDisplay::NoFrames.placeholder(), "--:--:--");
        assert_eq!(TimecodeDisplay::Hours.placeholder(), "--:--");
        assert_eq!(TimecodeDisplay::Minutes.placeholder(), "--:--");
        assert_eq!(TimecodeDisplay::Frames.placeholder(), "-------");
        assert_eq!(TimecodeDisplay::Seconds.placeholder(), "-----.---");
        assert_eq!(TimecodeDisplay::Milliseconds.placeholder(), "--:--:--.---");
    }

    #[test]
    fn numeric_displays_handle_drop_frame() {
        let timecode = Timecode {
            hours: 0,
            minutes: 1,
            seconds: 0,
            frames: 2,
            fps: 30_000.0 / 1001.0,
            drop_frame: true,
        };
        assert_eq!(TimecodeDisplay::Frames.format(&timecode), "1800");
        assert_eq!(TimecodeDisplay::Seconds.format(&timecode), "60.060");
        assert_eq!(TimecodeDisplay::Milliseconds.format(&timecode), "00:01:00.067");
    }
}
