//! Pure timeshift math — no I/O, no clock access, no global state.
//!
//! A VOD's real start instant is shifted by a user chosen offset (hours) to
//! produce a simulated live window `[simStart, simEnd]`. Comparing that window
//! against "now" yields the streamer state (live / upcoming / offline) plus the
//! numbers the UI needs to render progress and countdowns.
//!
//! All comparisons happen on UTC instants; the configured IANA timezone only
//! affects display formatting on the frontend.

use chrono::{DateTime, Duration as ChronoDuration, TimeDelta, Utc};

/// Upper bound for the user-selectable timeshift offset (one week).
pub const MAX_OFFSET_HOURS: f64 = 168.0;

/// State derived from comparing `now` with the simulated live window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeshiftState {
    /// `simStart <= now < simEnd`
    Live,
    /// `now < simStart`
    Upcoming,
    /// `now >= simEnd`
    Offline,
}

/// Result of [`evaluate`].
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub state: TimeshiftState,
    /// Simulated window start (`vod_start + offset`), always present.
    pub sim_start: DateTime<Utc>,
    /// Simulated window end (`sim_start + duration`), always present.
    pub sim_end: DateTime<Utc>,
    /// Seconds into the VOD if opened now — live only.
    pub join_seconds: Option<i64>,
    /// Seconds until the simulated start — upcoming only.
    pub starts_in_seconds: Option<i64>,
    /// `join_seconds / duration`, clamped to `0.0..=1.0` — live only.
    pub progress: Option<f64>,
}

/// Parse a Twitch VOD duration such as `3h25m10s`, `45m`, `90s` or `1h0m0s`.
///
/// Accepts the `d`/`h`/`m`/`s` units in descending order (Twitch itself emits
/// `h`/`m`/`s`, `d` is tolerated for very long broadcasts). Returns `None` for
/// anything malformed, out of order, duplicated or overflowing.
pub fn parse_duration(value: &str) -> Option<u64> {
    let text = value.trim();
    if text.is_empty() {
        return None;
    }

    let mut total: u64 = 0;
    let mut number: Option<u64> = None;
    let mut previous_rank: u32 = u32::MAX;
    let mut saw_unit = false;

    for ch in text.chars() {
        if let Some(digit) = ch.to_digit(10) {
            number = Some(
                number
                    .unwrap_or(0)
                    .checked_mul(10)?
                    .checked_add(digit as u64)?,
            );
            continue;
        }

        let amount = number.take()?;
        let (unit_seconds, rank): (u64, u32) = match ch {
            'd' => (86_400, 4),
            'h' => (3_600, 3),
            'm' => (60, 2),
            's' => (1, 1),
            _ => return None,
        };
        // Units must appear at most once and in descending order.
        if rank >= previous_rank {
            return None;
        }
        previous_rank = rank;
        total = total.checked_add(amount.checked_mul(unit_seconds)?)?;
        saw_unit = true;
    }

    // A trailing number without a unit (`"10"`) is not a duration.
    if number.is_some() || !saw_unit {
        None
    } else {
        Some(total)
    }
}

/// Convert a fractional hour offset into whole milliseconds.
fn offset_millis(offset_hours: f64) -> i64 {
    let millis = offset_hours * 3_600_000.0;
    if millis.is_finite() {
        millis.round() as i64
    } else {
        0
    }
}

fn add_offset(base: DateTime<Utc>, millis: i64) -> DateTime<Utc> {
    let fallback = if millis >= 0 {
        DateTime::<Utc>::MAX_UTC
    } else {
        DateTime::<Utc>::MIN_UTC
    };
    base.checked_add_signed(TimeDelta::milliseconds(millis))
        .unwrap_or(fallback)
}

/// Evaluate the simulated live window of a VOD.
///
/// * `now` — current instant (UTC).
/// * `vod_start` — the VOD's real start instant (UTC).
/// * `duration_seconds` — VOD length in seconds.
/// * `offset_hours` — user-chosen shift, may be fractional.
pub fn evaluate(
    now: DateTime<Utc>,
    vod_start: DateTime<Utc>,
    duration_seconds: u64,
    offset_hours: f64,
) -> Evaluation {
    let sim_start = add_offset(vod_start, offset_millis(offset_hours));
    let duration = i64::try_from(duration_seconds)
        .ok()
        .and_then(ChronoDuration::try_seconds)
        .unwrap_or(ChronoDuration::MAX);
    let sim_end = sim_start
        .checked_add_signed(duration)
        .unwrap_or(DateTime::<Utc>::MAX_UTC);

    if now < sim_start {
        // Round up so a countdown never reports "0 seconds" while upcoming.
        let starts_in = ceil_seconds(sim_start - now);
        Evaluation {
            state: TimeshiftState::Upcoming,
            sim_start,
            sim_end,
            join_seconds: None,
            starts_in_seconds: Some(starts_in),
            progress: None,
        }
    } else if now < sim_end {
        let join_seconds = (now - sim_start).num_seconds().max(0);
        let progress = if duration_seconds == 0 {
            0.0
        } else {
            (join_seconds as f64 / duration_seconds as f64).clamp(0.0, 1.0)
        };
        Evaluation {
            state: TimeshiftState::Live,
            sim_start,
            sim_end,
            join_seconds: Some(join_seconds),
            starts_in_seconds: None,
            progress: Some(progress),
        }
    } else {
        Evaluation {
            state: TimeshiftState::Offline,
            sim_start,
            sim_end,
            join_seconds: None,
            starts_in_seconds: None,
            progress: None,
        }
    }
}

/// A chosen candidate plus its evaluated window.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowSelection {
    /// Index into the `candidates` slice passed to [`select_window`].
    pub index: usize,
    pub evaluation: Evaluation,
}

/// Choose which broadcast a streamer card should show.
///
/// Candidates are `(real start, duration seconds)` pairs in any order.
/// Selection is deterministic:
///
/// 1. A currently **live** shifted window beats everything. Overlapping live
///    windows resolve to the latest `sim_start` (the most recently started
///    replay), then the longer duration, then the earliest list position.
/// 2. Otherwise the **nearest upcoming** window (smallest `sim_start`).
/// 3. Otherwise the **most recently ended** window (largest `sim_end`).
pub fn select_window(
    candidates: &[(DateTime<Utc>, u64)],
    now: DateTime<Utc>,
    offset_hours: f64,
) -> Option<WindowSelection> {
    let mut live: Option<WindowSelection> = None;
    let mut upcoming: Option<WindowSelection> = None;
    let mut offline: Option<WindowSelection> = None;

    for (index, &(start, duration)) in candidates.iter().enumerate() {
        let evaluation = evaluate(now, start, duration, offset_hours);
        let candidate = WindowSelection { index, evaluation };
        match candidate.evaluation.state {
            TimeshiftState::Live => {
                let wins = match &live {
                    None => true,
                    Some(current) => {
                        let cur_dur = candidates[current.index].1;
                        (candidate.evaluation.sim_start, duration)
                            > (current.evaluation.sim_start, cur_dur)
                    }
                };
                if wins {
                    live = Some(candidate);
                }
            }
            TimeshiftState::Upcoming => {
                let wins = match &upcoming {
                    None => true,
                    Some(current) => candidate.evaluation.sim_start < current.evaluation.sim_start,
                };
                if wins {
                    upcoming = Some(candidate);
                }
            }
            TimeshiftState::Offline => {
                let wins = match &offline {
                    None => true,
                    Some(current) => {
                        (candidate.evaluation.sim_end, candidate.evaluation.sim_start)
                            > (current.evaluation.sim_end, current.evaluation.sim_start)
                    }
                };
                if wins {
                    offline = Some(candidate);
                }
            }
        }
    }

    live.or(upcoming).or(offline)
}

fn ceil_seconds(span: ChronoDuration) -> i64 {
    let millis = span.num_milliseconds();
    let mut seconds = millis / 1_000;
    if millis % 1_000 != 0 {
        seconds += 1;
    }
    seconds.max(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn at(hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 3, 1, hour, minute, second)
            .unwrap()
    }

    fn vod_start() -> DateTime<Utc> {
        at(12, 0, 0)
    }

    #[test]
    fn duration_parse_table() {
        let table: &[(&str, Option<u64>)] = &[
            ("3h25m10s", Some(12_310)),
            ("45m", Some(2_700)),
            ("90s", Some(90)),
            ("1h0m0s", Some(3_600)),
            ("0s", Some(0)),
            ("1h", Some(3_600)),
            ("1h30m", Some(5_400)),
            ("2d3h4m5s", Some(2 * 86_400 + 3 * 3_600 + 4 * 60 + 5)),
            ("  45m  ", Some(2_700)),
            ("", None),
            ("   ", None),
            ("10", None),
            ("abc", None),
            ("h", None),
            ("3x", None),
            ("3h 25m", None),
            ("3h25m10", None),
            ("1h1h", None),
            ("1m1h", None),
            ("-90s", None),
            ("1.5h", None),
            ("10s5", None),
        ];
        for (input, expected) in table {
            assert_eq!(parse_duration(input), *expected, "input: {input:?}");
        }
    }

    #[test]
    fn before_window_is_upcoming() {
        // offset 24h: the window starts a full day after the real broadcast.
        // 23h after the real start is exactly one hour before the sim start.
        let now = vod_start() + ChronoDuration::hours(23);
        let result = evaluate(now, vod_start(), 3_600, 24.0);

        assert_eq!(result.state, TimeshiftState::Upcoming);
        assert_eq!(result.sim_start, vod_start() + ChronoDuration::hours(24));
        assert_eq!(result.sim_end, result.sim_start + ChronoDuration::hours(1));
        assert_eq!(result.starts_in_seconds, Some(3_600));
        assert_eq!(result.join_seconds, None);
        assert_eq!(result.progress, None);
    }

    #[test]
    fn exact_start_is_live_with_zero_join() {
        let start = vod_start() + ChronoDuration::hours(24);
        let result = evaluate(start, vod_start(), 3_600, 24.0);

        assert_eq!(result.state, TimeshiftState::Live);
        assert_eq!(result.join_seconds, Some(0));
        assert_eq!(result.progress, Some(0.0));
        assert_eq!(result.starts_in_seconds, None);
    }

    #[test]
    fn mid_stream_progress() {
        let start = vod_start() + ChronoDuration::hours(24);
        let now = start + ChronoDuration::minutes(30);
        let result = evaluate(now, vod_start(), 3_600, 24.0);

        assert_eq!(result.state, TimeshiftState::Live);
        assert_eq!(result.join_seconds, Some(1_800));
        assert_eq!(result.progress, Some(0.5));
        assert_eq!(result.sim_end, start + ChronoDuration::hours(1));
    }

    #[test]
    fn exact_end_is_offline() {
        let end = vod_start() + ChronoDuration::hours(24) + ChronoDuration::hours(1);
        let result = evaluate(end, vod_start(), 3_600, 24.0);

        assert_eq!(result.state, TimeshiftState::Offline);
        assert_eq!(result.join_seconds, None);
        assert_eq!(result.starts_in_seconds, None);
        assert_eq!(result.progress, None);
    }

    #[test]
    fn fractional_offset_shifts_the_window() {
        // 1.5h offset => sim start = real start + 01:30:00.
        let start = vod_start() + ChronoDuration::seconds(5_400);
        assert_eq!(
            evaluate(start, vod_start(), 1_800, 1.5).state,
            TimeshiftState::Live
        );

        let result = evaluate(start + ChronoDuration::minutes(10), vod_start(), 1_800, 1.5);
        assert_eq!(result.sim_start, start);
        assert_eq!(result.sim_end, start + ChronoDuration::seconds(1_800));
        assert_eq!(result.join_seconds, Some(600));
        assert_eq!(result.progress, Some(600.0 / 1_800.0));

        // Fractional offsets with sub-second precision are rounded to millis.
        // 0.25h = 15min after the real start, so the real start is upcoming.
        let sim = vod_start() + ChronoDuration::seconds(900);
        let quarter = evaluate(vod_start(), vod_start(), 60, 0.25);
        assert_eq!(quarter.sim_start, sim);
        assert_eq!(quarter.state, TimeshiftState::Upcoming);
        assert_eq!(quarter.starts_in_seconds, Some(900));
        assert_eq!(
            evaluate(sim, vod_start(), 60, 0.25).state,
            TimeshiftState::Live
        );
    }

    #[test]
    fn progress_is_clamped_and_zero_length_is_offline() {
        // A zero-length VOD has an empty window: its start is already the end.
        let zero = evaluate(vod_start(), vod_start(), 0, 0.0);
        assert_eq!(zero.state, TimeshiftState::Offline);

        let result = evaluate(
            vod_start() + ChronoDuration::seconds(30),
            vod_start(),
            60,
            0.0,
        );
        assert_eq!(result.progress, Some(0.5));
        assert!(result.progress.unwrap() >= 0.0 && result.progress.unwrap() <= 1.0);
    }

    #[test]
    fn offset_is_added_to_real_start_not_to_now() {
        // 0.5h offset on a 10 minute VOD: window opens at 12:30.
        let result = evaluate(at(12, 15, 0), vod_start(), 600, 0.5);
        assert_eq!(
            result.sim_start,
            vod_start() + ChronoDuration::seconds(1_800)
        );
        assert_eq!(result.state, TimeshiftState::Upcoming);
        assert_eq!(result.starts_in_seconds, Some(900));
    }

    #[test]
    fn non_finite_offset_is_treated_as_zero() {
        let result = evaluate(vod_start(), vod_start(), 60, f64::NAN);
        assert_eq!(result.sim_start, vod_start());
        assert_eq!(result.state, TimeshiftState::Live);
        assert_eq!(result.join_seconds, Some(0));
    }

    #[test]
    fn selection_prefers_active_replay_over_newer_vod() {
        // Regression (BUG_REVIEW #1): yesterday's 4h stream started 18:00.
        // At +24h it airs today 18:00-22:00. A newer VOD from today must not
        // hide that active replay.
        let yesterday_18 = Utc.with_ymd_and_hms(2024, 3, 1, 18, 0, 0).unwrap();
        let today_19 = Utc.with_ymd_and_hms(2024, 3, 2, 19, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2024, 3, 2, 19, 0, 0).unwrap();
        // Newest first, as the API returns them.
        let candidates = [
            (today_19, 3 * 3_600),     // today's stream -> sim tomorrow 19:00
            (yesterday_18, 4 * 3_600), // yesterday's -> sim today 18:00-22:00
        ];
        let selected = select_window(&candidates, now, 24.0).expect("a candidate");
        assert_eq!(selected.index, 1);
        assert_eq!(selected.evaluation.state, TimeshiftState::Live);
        assert_eq!(selected.evaluation.join_seconds, Some(3_600));
        assert_eq!(
            selected.evaluation.sim_start,
            Utc.with_ymd_and_hms(2024, 3, 2, 18, 0, 0).unwrap()
        );
        assert_eq!(
            selected.evaluation.sim_end,
            Utc.with_ymd_and_hms(2024, 3, 2, 22, 0, 0).unwrap()
        );
    }

    #[test]
    fn selection_overlap_picks_latest_start() {
        let now = at(15, 0, 0);
        // Both live at 15:00 with offset 0: A 10:00-18:00, B 14:00-16:00.
        let candidates = [(at(14, 0, 0), 2 * 3_600), (at(10, 0, 0), 8 * 3_600)];
        let selected = select_window(&candidates, now, 0.0).expect("a candidate");
        assert_eq!(selected.index, 0, "later sim_start wins the overlap");
        assert_eq!(selected.evaluation.join_seconds, Some(3_600));
    }

    #[test]
    fn selection_upcoming_picks_nearest_start() {
        let now = at(12, 0, 0);
        let candidates = [(at(20, 0, 0), 3_600), (at(13, 30, 0), 3_600)];
        let selected = select_window(&candidates, now, 0.0).expect("a candidate");
        assert_eq!(selected.index, 1);
        assert_eq!(selected.evaluation.state, TimeshiftState::Upcoming);
        assert_eq!(selected.evaluation.starts_in_seconds, Some(5_400));
    }

    #[test]
    fn selection_all_past_picks_most_recently_ended() {
        let now = at(23, 0, 0);
        let candidates = [(at(20, 0, 0), 3_600), (at(8, 0, 0), 7_200)];
        let selected = select_window(&candidates, now, 0.0).expect("a candidate");
        assert_eq!(selected.index, 0);
        assert_eq!(selected.evaluation.state, TimeshiftState::Offline);
    }

    #[test]
    fn selection_empty_is_none() {
        assert!(select_window(&[], at(12, 0, 0), 24.0).is_none());
    }

    #[test]
    fn absurd_inputs_do_not_panic() {
        // f64::MAX hours overflows to a non-finite millisecond value and is
        // treated like a zero offset; the huge duration saturates to MAX_UTC.
        let huge = evaluate(vod_start(), vod_start(), u64::MAX, f64::MAX);
        assert_eq!(huge.sim_start, vod_start());
        assert_eq!(huge.sim_end, DateTime::<Utc>::MAX_UTC);
        assert_eq!(huge.state, TimeshiftState::Live);
        assert_eq!(huge.join_seconds, Some(0));

        let negative = evaluate(vod_start(), vod_start(), 60, f64::NEG_INFINITY);
        assert_eq!(negative.sim_start, vod_start());
        assert_eq!(negative.state, TimeshiftState::Live);
    }
}
