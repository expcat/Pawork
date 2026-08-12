//! 五字段 cron 表达式的自实现最小子集解析与 `next_fire` 计算。
//!
//! 不引入任何调度框架。支持字段：`分 时 日 月 周`（范围见 [`FIELD_BOUNDS`]）。
//! 操作符：`*`、单个数字、`,` 列表、`-` 范围、`/` 步长，及其组合（`a-b/n`、`*/n`、`a/n`）。
//!
//! 时间以 Unix 秒为单位；匹配粒度为分钟。`next_fire(from)` 返回严格晚于 `from`
//! 的下一个匹配分钟起始时刻，便于在确定性调度中用注入的 `now` 测试。
//!
//! `day-of-month` 与 `day-of-week` 遵循 Vixie cron 语义：两者均受限（都不是
//! `*`）时取「或」关系，否则取「与」关系。

/// 五个字段的 `(min, max)` 边界，顺序：minute hour day-of-month month day-of-week。
const FIELD_BOUNDS: [(u32, u32); 5] = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 6)];

/// 单个字段的命中位图（按 `value - min` 索引）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct FieldBits {
    bits: Vec<bool>,
    min: u32,
    /// 原始字段是否受限（非 `*`）；用于 dom/dow 的或语义判定。
    restricted: bool,
}

impl FieldBits {
    fn new(min: u32, max: u32) -> Self {
        Self {
            bits: vec![false; (max - min + 1) as usize],
            min,
            restricted: false,
        }
    }

    fn set(&mut self, value: u32) {
        if let Some(slot) = self.bits.get_mut((value - self.min) as usize) {
            *slot = true;
        }
    }

    fn matches(&self, value: u32) -> bool {
        self.bits
            .get((value - self.min) as usize)
            .copied()
            .unwrap_or(false)
    }
}

/// 解析后的 cron 计划（五个字段的命中集合 + dom/dow 受限标记）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CronSchedule {
    minute: FieldBits,
    hour: FieldBits,
    dom: FieldBits,
    month: FieldBits,
    dow: FieldBits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CronFields {
    minute: u32,
    hour: u32,
    dom: u32,
    month: u32,
    dow: u32,
}

/// 解析单个字段为命中位图。`raw` 为原始字段文本（用于判定 `restricted`）。
fn parse_field(raw: &str, min: u32, max: u32) -> Result<FieldBits, String> {
    let mut field = FieldBits::new(min, max);
    field.restricted = raw.trim() != "*";

    for token in raw.split(',') {
        let token = token.trim();
        if token.is_empty() {
            return Err("empty list element".into());
        }
        let (range_part, step) = match token.split_once('/') {
            Some((range_part, step_part)) => {
                let step: u32 = step_part
                    .parse()
                    .map_err(|_| format!("invalid step `{step_part}`"))?;
                if step == 0 {
                    return Err("step must be > 0".into());
                }
                (range_part, Some(step))
            }
            None => (token, None),
        };

        let (lo, hi) = if range_part == "*" {
            (min, max)
        } else if let Some((start_part, end_part)) = range_part.split_once('-') {
            let start: u32 = start_part
                .parse()
                .map_err(|_| format!("invalid range start `{start_part}`"))?;
            let end: u32 = end_part
                .parse()
                .map_err(|_| format!("invalid range end `{end_part}`"))?;
            if start < min || end > max {
                return Err(format!("range {start}-{end} out of {min}-{max}"));
            }
            if start > end {
                return Err(format!("inverted range {start}-{end}"));
            }
            (start, end)
        } else {
            let value: u32 = range_part
                .parse()
                .map_err(|_| format!("invalid value `{range_part}`"))?;
            if value < min || value > max {
                return Err(format!("value {value} out of {min}-{max}"));
            }
            // `a/n` 语义：从 a 到字段上限，按 n 步进（与 Vixie cron 一致）。
            if step.is_some() {
                (value, max)
            } else {
                (value, value)
            }
        };

        let step = step.unwrap_or(1);
        let mut current = lo;
        while current <= hi {
            field.set(current);
            current = match current.checked_add(step) {
                Some(next) => next,
                None => break,
            };
        }
    }

    Ok(field)
}

impl CronSchedule {
    /// 解析五字段 cron 表达式。
    pub fn parse(expr: &str) -> Result<Self, String> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!("expected 5 fields, got {}", fields.len()));
        }
        let bounds = FIELD_BOUNDS;
        let parsed: Vec<FieldBits> = fields
            .iter()
            .zip(bounds.iter())
            .map(|(raw, &(min, max))| parse_field(raw, min, max))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            minute: parsed[0].clone(),
            hour: parsed[1].clone(),
            dom: parsed[2].clone(),
            month: parsed[3].clone(),
            dow: parsed[4].clone(),
        })
    }

    /// 返回严格晚于 `from` 的下一个匹配分钟起始时刻（Unix 秒）。
    ///
    /// 扫描上限为 `from` 之后约 4 年；超出仍无匹配返回 `None`（罕见，如不可能日期）。
    pub fn next_fire(&self, from: u64) -> Option<u64> {
        // 对齐到下一分钟起点（丢弃秒精度）。
        let mut candidate = (from / 60 + 1) * 60;
        let limit = from.saturating_add(SCAN_HORIZON_SECONDS);
        while candidate <= limit {
            let fields = breakdown(candidate);
           if !self.month.matches(fields.month) {
               // 整月不匹配：按天推进，快速跳过不可能的月份。
                candidate = next_day_start(candidate);
               continue;
           }
            let dom_match = self.dom.matches(fields.dom);
            let dow_match = self.dow.matches(fields.dow);
            let day_ok = if self.dom.restricted && self.dow.restricted {
                dom_match || dow_match
            } else {
                dom_match && dow_match
            };
           if !day_ok {
                candidate = next_day_start(candidate);
               continue;
           }
            if !self.hour.matches(fields.hour) {
                // 跳到下一个小时起点。
                candidate = (candidate / SECONDS_PER_HOUR + 1) * SECONDS_PER_HOUR;
                continue;
            }
            if !self.minute.matches(fields.minute) {
                candidate = candidate.saturating_add(60);
                continue;
            }
            return Some(candidate);
        }
        None
    }
}

const SECONDS_PER_HOUR: u64 = 3_600;
const SECONDS_PER_DAY: u64 = 86_400;
/// 约 4 * 366 天，覆盖闰年与罕见组合的扫描上限。
const SCAN_HORIZON_SECONDS: u64 = 4 * 366 * SECONDS_PER_DAY;

/// 返回 `t` 之后下一个 UTC 日（00:00:00）的起点。
fn next_day_start(t: u64) -> u64 {
    ((t / SECONDS_PER_DAY) + 1) * SECONDS_PER_DAY
}

/// 把 Unix 秒分解为 cron 字段（UTC）。`dow` 0=Sunday。
fn breakdown(timestamp: u64) -> CronFields {
    let days = (timestamp / SECONDS_PER_DAY) as i64;
    let secs_of_day = timestamp % SECONDS_PER_DAY;
    let hour = (secs_of_day / SECONDS_PER_HOUR) as u32;
    let minute = ((secs_of_day % SECONDS_PER_HOUR) / 60) as u32;
    let dow = ((days + 4).rem_euclid(7)) as u32;
    let (_year, month, dom) = civil_from_days(days);
    CronFields {
        minute,
        hour,
        dom,
        month,
        dow,
    }
}

/// Howard Hinnant 的 `civil_from_days`：日数 → (年, 月 1-12, 日 1-31)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

/// 便利函数：解析表达式并返回严格晚于 `now` 的下一次触发时刻。
///
/// 解析失败或扫描窗口内无匹配时返回 `None`。
pub fn next_fire(expr: &str, now: u64) -> Option<u64> {
    CronSchedule::parse(expr)
        .ok()
        .and_then(|schedule| schedule.next_fire(now))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2024-01-01T00:00:00Z 的 Unix 秒（周一）。
    const BASE: u64 = 1_704_067_200;

    fn at(year: i64, month: u32, day: u32, hour: u32, minute: u32) -> u64 {
        // 构造指定 UTC 时刻的 Unix 秒，用于断言 next_fire 落点。
        let days = days_from_civil(year, month, day);
        (days as u64) * SECONDS_PER_DAY
            + (hour as u64) * SECONDS_PER_HOUR
            + (minute as u64) * 60
    }

    fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
        let y = if month <= 2 { year - 1 } else { year };
        let era = (if y >= 0 { y } else { y - 399 }) / 400;
        let yoe = y - era * 400;
        let m = month as i64;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + (day as i64) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    #[test]
    fn every_minute_fires_next_minute() {
        let schedule = CronSchedule::parse("* * * * *").unwrap();
        assert_eq!(schedule.next_fire(BASE), Some(BASE + 60));
    }

    #[test]
    fn specific_minute_hour_same_day() {
        // 每天 09:30；BASE=00:00，下一次为当天 09:30。
        let schedule = CronSchedule::parse("30 9 * * *").unwrap();
        assert_eq!(schedule.next_fire(BASE), Some(at(2024, 1, 1, 9, 30)));
    }

    #[test]
    fn specific_minute_hour_next_day_when_past() {
        // 09:30 在 BASE(00:00) 之后 09:30 当天；从 10:00 起应落到次日 09:30。
        let schedule = CronSchedule::parse("30 9 * * *").unwrap();
        let from = at(2024, 1, 1, 10, 0);
        assert_eq!(schedule.next_fire(from), Some(at(2024, 1, 2, 9, 30)));
    }

    #[test]
    fn step_minute_hits_correct_slots() {
        // 每 15 分钟：0/15/30/45。
        let schedule = CronSchedule::parse("*/15 * * * *").unwrap();
        assert_eq!(schedule.next_fire(BASE), Some(BASE + 15 * 60));
        assert_eq!(
            schedule.next_fire(BASE + 15 * 60),
            Some(BASE + 30 * 60)
        );
    }

    #[test]
    fn range_with_step() {
        // 9 点到 17 点每 2 小时：9,11,13,15,17。
        let schedule = CronSchedule::parse("0 9-17/2 * * *").unwrap();
        assert_eq!(schedule.next_fire(BASE), Some(at(2024, 1, 1, 9, 0)));
        assert_eq!(
            schedule.next_fire(at(2024, 1, 1, 9, 0)),
            Some(at(2024, 1, 1, 11, 0))
        );
    }

    #[test]
    fn list_field() {
        // 每小时的第 0、15、30、45 分钟之外的第 5 和第 20 分钟。
        let schedule = CronSchedule::parse("5,20 * * * *").unwrap();
        assert_eq!(schedule.next_fire(BASE), Some(BASE + 5 * 60));
        assert_eq!(
            schedule.next_fire(BASE + 5 * 60),
            Some(BASE + 20 * 60)
        );
    }

    #[test]
    fn specific_month() {
        // 仅 3 月每天的 00:00。
        let schedule = CronSchedule::parse("0 0 * 3 *").unwrap();
        assert_eq!(schedule.next_fire(BASE), Some(at(2024, 3, 1, 0, 0)));
    }

    #[test]
    fn day_of_week_sunday() {
        // 每周日 12:00。2024-01-01 是周一；首个周日是 2024-01-07。
        let schedule = CronSchedule::parse("0 12 * * 0").unwrap();
        assert_eq!(schedule.next_fire(BASE), Some(at(2024, 1, 7, 12, 0)));
    }

    #[test]
    fn dom_and_dow_or_semantics_when_both_restricted() {
        // dom=15 且 dow=0（周日）：两者都受限 → 任一命中即触发。
        // 2024-01-15 是周一（dom 命中），应为 2024-01-15 00:00。
       let schedule = CronSchedule::parse("0 0 15 * 0").unwrap();
        // BASE(2024-01-01 Mon) 起：dom=15 与 dow=0(周日) 均受限 → OR 语义取较早命中。
       // 首个周日 2024-01-07(dow 命中) 早于 dom=15(01-15 周一)，故首触发为 01-07。
       assert_eq!(schedule.next_fire(BASE), Some(at(2024, 1, 7, 0, 0)));
        // 01-07 之后：OR 语义下下一个命中是 01-14（周日 dow 命中），再下个 01-15（dom 命中）。
        assert_eq!(
            schedule.next_fire(at(2024, 1, 7, 0, 0)),
            Some(at(2024, 1, 14, 0, 0))
        );
        assert_eq!(
            schedule.next_fire(at(2024, 1, 14, 0, 0)),
            Some(at(2024, 1, 15, 0, 0))
        );
    }

    #[test]
    fn february_30_is_impossible_returns_none_or_skips() {
        // 2 月 30 日不存在；扫描窗口内无匹配应返回 None。
        let schedule = CronSchedule::parse("0 0 30 2 *").unwrap();
        assert_eq!(schedule.next_fire(BASE), None);
    }

    #[test]
    fn rejects_wrong_field_count() {
        assert!(CronSchedule::parse("* * * *").is_err());
        assert!(CronSchedule::parse("* * * * * *").is_err());
    }

    #[test]
    fn rejects_out_of_range_values() {
        assert!(CronSchedule::parse("60 * * * *").is_err());
        assert!(CronSchedule::parse("* 24 * * *").is_err());
        assert!(CronSchedule::parse("* * 32 * *").is_err());
        assert!(CronSchedule::parse("* * * 13 *").is_err());
        assert!(CronSchedule::parse("* * * * 7").is_err());
    }

    #[test]
    fn rejects_zero_step() {
        assert!(CronSchedule::parse("*/0 * * * *").is_err());
    }

    #[test]
    fn convenience_next_fire_parses_and_computes() {
        assert_eq!(next_fire("0 0 * * *", BASE), Some(at(2024, 1, 2, 0, 0)));
        assert_eq!(next_fire("bad expr", BASE), None);
    }
}
