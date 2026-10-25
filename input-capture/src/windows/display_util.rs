use windows::Win32::Foundation::RECT;

use crate::Position;
use mousehop_ipc::WindowsCrossingZone;

fn is_within_dp_region(point: (i32, i32), display: &RECT) -> bool {
    [
        Position::Left,
        Position::Right,
        Position::Top,
        Position::Bottom,
    ]
    .iter()
    .all(|&pos| is_within_dp_boundary(point, display, pos))
}

fn is_within_dp_boundary(point: (i32, i32), display: &RECT, pos: Position) -> bool {
    let (x, y) = point;
    match pos {
        Position::Left => display.left <= x,
        Position::Right => display.right > x,
        Position::Top => display.top <= y,
        Position::Bottom => display.bottom > y,
    }
}

/// returns whether the given position is within the display bounds with respect to the given
/// barrier position
///
/// # Arguments
///
/// * `x`:
/// * `y`:
/// * `displays`:
/// * `pos`:
///
/// returns: bool
///
fn in_bounds(point: (i32, i32), displays: &[RECT], pos: Position) -> bool {
    displays
        .iter()
        .any(|d| is_within_dp_boundary(point, d, pos))
}

fn in_display_region(point: (i32, i32), displays: &[RECT]) -> bool {
    displays.iter().any(|d| is_within_dp_region(point, d))
}

fn moved_across_boundary(
    prev_pos: (i32, i32),
    curr_pos: (i32, i32),
    displays: &[RECT],
    pos: Position,
) -> bool {
    /* was within bounds, but is not anymore */
    in_display_region(prev_pos, displays) && !in_bounds(curr_pos, displays, pos)
}

pub(crate) fn entered_barrier(
    prev_pos: (i32, i32),
    curr_pos: (i32, i32),
    displays: &[RECT],
) -> Option<Position> {
    [
        Position::Left,
        Position::Right,
        Position::Top,
        Position::Bottom,
    ]
    .into_iter()
    .find(|&pos| moved_across_boundary(prev_pos, curr_pos, displays, pos))
}

///
/// clamp point to display bounds
///
/// # Arguments
///
/// * `prev_point`: coordinates, the cursor was before entering, within bounds of a display
/// * `entry_point`: point to clamp
///
/// returns: (i32, i32), the corrected entry point
///
pub(crate) fn clamp_to_display_bounds(
    display_regions: &[RECT],
    prev_point: (i32, i32),
    point: (i32, i32),
) -> (i32, i32) {
    /* find display where movement came from */
    let display = display_regions
        .iter()
        .find(|&d| is_within_dp_region(prev_point, d))
        .unwrap();

    /* clamp to bounds (inclusive) */
    let (x, y) = point;
    let (min_x, max_x) = (display.left, display.right - 1);
    let (min_y, max_y) = (display.top, display.bottom - 1);
    (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
}

/// Find a configured monitor-edge crossing. Unlike the legacy global-edge
/// test, this can activate a different logical peer direction than the physical
/// edge crossed (e.g. left edge of a low side monitor -> bottom peer).
///
/// The crossing must *leave* the Windows layout. Traversing directly between
/// overlapping display regions keeps control in Windows.
pub(crate) fn entered_monitor_zone(
    prev: (i32, i32),
    curr: (i32, i32),
    displays: &[RECT],
    names: &[String],
    zones: &[WindowsCrossingZone],
) -> Option<Position> {
    if !in_display_region(prev, displays) || in_display_region(curr, displays) {
        return None;
    }

    zones.iter().find_map(|zone| {
        let name = zone.monitor.rsplit('\\').next().unwrap_or(&zone.monitor);
        let index = names.iter().position(|n| {
            n.rsplit('\\').next().unwrap_or(n).eq_ignore_ascii_case(name)
        })?;
        let display = displays.get(index)?;
        if !is_within_dp_region(prev, display) {
            return None;
        }

        // Calculate intersection with the physical edge, rather than using
        // the destination point: diagonal motion must not select a zone on
        // another part of the monitor.
        let (from, to, boundary, origin, length) = match zone.edge {
            Position::Left if curr.0 < display.left => (
                prev.0, curr.0, display.left, display.top, display.bottom - display.top,
            ),
            Position::Right if curr.0 >= display.right => (
                prev.0, curr.0, display.right, display.top, display.bottom - display.top,
            ),
            Position::Top if curr.1 < display.top => (
                prev.1, curr.1, display.top, display.left, display.right - display.left,
            ),
            Position::Bottom if curr.1 >= display.bottom => (
                prev.1, curr.1, display.bottom, display.left, display.right - display.left,
            ),
            _ => return None,
        };
        if to == from || length <= 0 {
            return None;
        }
        let t = (f64::from(boundary) - f64::from(from))
            / (f64::from(to) - f64::from(from));
        if !(0.0..=1.0).contains(&t) {
            return None;
        }
        let along = match zone.edge {
            Position::Left | Position::Right => {
                f64::from(prev.1) + t * f64::from(curr.1 - prev.1)
            }
            Position::Top | Position::Bottom => {
                f64::from(prev.0) + t * f64::from(curr.0 - prev.0)
            }
        } - f64::from(origin);
        if along < 0.0 || along >= f64::from(length) {
            return None;
        }
        let start = range_offset(zone.start_px, zone.start_percent, 0.0, length)?;
        let end = range_offset(zone.end_px, zone.end_percent, 100.0, length)?;
        (start < end && along >= start && along < end).then_some(zone.target)
    })
}

fn range_offset(pixels: Option<u32>, percent: Option<f64>, default: f64, length: i32) -> Option<f64> {
    if let Some(px) = pixels {
        return Some(f64::from(px));
    }
    let p = percent.unwrap_or(default);
    (p.is_finite() && (0.0..=100.0).contains(&p)).then_some(f64::from(length) * p / 100.0)
}

#[cfg(test)]
mod monitor_zone_tests {
    use super::*;

    fn zone(monitor: &str, edge: Position, target: Position, start: f64, end: f64) -> WindowsCrossingZone {
        WindowsCrossingZone {
            monitor: monitor.to_owned(),
            edge,
            target,
            start_px: None,
            end_px: None,
            start_percent: Some(start),
            end_percent: Some(end),
        }
    }

    #[test]
    fn a_side_monitor_can_cross_only_from_its_lower_half() {
        let displays = [
            RECT { left: 0, top: 0, right: 1000, bottom: 800 },
            RECT { left: 1000, top: 0, right: 2000, bottom: 800 },
            RECT { left: 2000, top: 0, right: 3000, bottom: 1600 },
        ];
        let names = vec!["DISPLAY1".into(), "DISPLAY2".into(), r"\\.\DISPLAY3".into()];
        let zones = vec![zone("DISPLAY3", Position::Left, Position::Bottom, 50.0, 100.0)];

        // Upper half of the right display connects to the center Windows display.
        assert_eq!(entered_monitor_zone((2005, 200), (1990, 200), &displays, &names, &zones), None);
        // The lower half has no adjacent Windows display and activates Linux.
        assert_eq!(entered_monitor_zone((2005, 1200), (1990, 1200), &displays, &names, &zones), Some(Position::Bottom));
        // Unlisted monitor boundaries are never active.
        assert_eq!(entered_monitor_zone((1400, 790), (1400, 810), &displays, &names, &zones), None);
        assert_eq!(entered_monitor_zone((2005, 200), (1980, 200), &displays, &names, &zones), None);
    }

    #[test]
    fn pixel_limits_are_exclusive_at_the_end() {
        let displays = [RECT { left: 0, top: 0, right: 1000, bottom: 800 }];
        let names = vec!["DISPLAY1".into()];
        let mut z = zone("DISPLAY1", Position::Bottom, Position::Bottom, 0.0, 100.0);
        z.start_px = Some(250);
        z.end_px = Some(750);
        assert_eq!(entered_monitor_zone((250, 795), (250, 805), &displays, &names, &[z.clone()]), Some(Position::Bottom));
        assert_eq!(entered_monitor_zone((750, 795), (750, 805), &displays, &names, &[z]), None);
    }

    #[test]
    fn unknown_monitor_name_never_activates_capture() {
        let displays = [RECT {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 800,
        }];
        let names = vec!["DISPLAY1".into()];
        let zones = vec![zone(
            "DISPLAY9",
            Position::Bottom,
            Position::Bottom,
            0.0,
            100.0,
        )];

        assert_eq!(
            entered_monitor_zone((500, 795), (500, 805), &displays, &names, &zones),
            None,
        );
    }

    #[test]
    fn invalid_percentage_rejected_and_diagonal_path_respects_edge() {
        let displays = [RECT { left: 0, top: 0, right: 1000, bottom: 800 }];
        let names = vec!["DISPLAY1".into()];
        let mut z = zone("DISPLAY1", Position::Right, Position::Right, 0.0, 100.0);
        z.end_percent = Some(f64::NAN);
        assert_eq!(entered_monitor_zone((990, 500), (1010, 500), &displays, &names, &[z]), None);
        let z = zone("DISPLAY1", Position::Right, Position::Right, 0.0, 50.0);
        assert_eq!(entered_monitor_zone((990, 390), (1010, 420), &displays, &names, &[z]), None);
    }
}
