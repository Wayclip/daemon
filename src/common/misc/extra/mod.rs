// TODO: might end up moving to core
use chrono::{DateTime, Local, TimeDelta};

#[derive(Default)]
pub enum WayclipTimeTimezone {
    #[default]
    Local,
    UTC,
}

#[derive(Default)]
pub enum FormatMode {
    Exhaustive,
    #[default]
    Default,
    Minimal,
}

impl FormatMode {
    pub fn get_formatting_string(&self) -> &'static str {
        match self {
            // Wednesday, September 23, 2026 at 07:05:42 PM ICT (+07:00)
            FormatMode::Exhaustive => "%A, %B %-d, %Y at %I:%M:%S %p %Z (%:z)",
            // Sep 23, 2026, 07:05 PM ICT
            FormatMode::Default => "%b %-d, %Y, %I:%M %p %Z",
            // 09/23/26 19:05
            FormatMode::Minimal => "%m/%d/%y %H:%M",
        }
    }

    pub fn format_timedelta(&self, delta: TimeDelta) -> String {
        let total_seconds = delta.num_seconds().max(0);
        let days = total_seconds / 86_400;
        let hours = (total_seconds % 86_400) / 3_600;
        let minutes = (total_seconds % 3_600) / 60;
        let seconds = total_seconds % 60;

        match self {
            FormatMode::Exhaustive => {
                format!("{days} days, {hours} hours, {minutes} minutes, {seconds} seconds")
            }
            FormatMode::Default => {
                if days > 0 {
                    format!("{days}d {hours}h {minutes}m {seconds}s")
                } else if hours > 0 {
                    format!("{hours}h {minutes}m {seconds}s")
                } else {
                    format!("{minutes}m {seconds}s")
                }
            }
            FormatMode::Minimal => {
                if days > 0 {
                    format!("{days}d {hours}h")
                } else if hours > 0 {
                    format!("{hours}h {minutes}m")
                } else if minutes > 0 {
                    format!("{minutes}m {seconds}s")
                } else {
                    format!("{seconds}s")
                }
            }
        }
    }
}

/// A struct built around the need of not importing chrono to perform calculations and formatting
/// with time
///
/// Instead, create a WayclipTime object with the specified timezone. Then you can retreive the
/// tiemstamp, formatted string or calculate DeltaTime
pub struct WayclipTime {
    tz: WayclipTimeTimezone,
}

impl WayclipTime {
    pub fn get_utc() -> DateTime<chrono::Utc> {
        chrono::Utc::now()
    }

    pub fn get_local() -> DateTime<chrono::Local> {
        chrono::Local::now()
    }

    pub fn new(tz: WayclipTimeTimezone) -> Self {
        Self { tz }
    }

    pub fn now_timestamp(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }

    pub fn now_formatted_string(&self, mode: FormatMode) -> String {
        let format = mode.get_formatting_string();
        match self.tz {
            WayclipTimeTimezone::UTC => chrono::Utc::now().format(format).to_string(),
            WayclipTimeTimezone::Local => chrono::Local::now().format(format).to_string(),
        }
    }

    pub fn get_time_delta(&self, past_timestamp_secs: i64) -> TimeDelta {
        TimeDelta::seconds(self.now_timestamp() - past_timestamp_secs)
    }

    pub fn get_time_delta_formatted(&self, past_timestamp_secs: i64, mode: FormatMode) -> String {
        let delta = self.get_time_delta(past_timestamp_secs);
        mode.format_timedelta(delta)
    }

    pub fn format_timestamp(&self, timestamp_secs: i64, mode: FormatMode) -> String {
        let fmt = mode.get_formatting_string();
        let dt_utc = DateTime::from_timestamp(timestamp_secs, 0).unwrap_or(DateTime::default());

        match self.tz {
            WayclipTimeTimezone::UTC => dt_utc.format(fmt).to_string(),
            WayclipTimeTimezone::Local => dt_utc.with_timezone(&Local).format(fmt).to_string(),
        }
    }
}
