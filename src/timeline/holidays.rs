use chrono::{Datelike, NaiveDate};
use lunar_lite::{solar_to_lunar, SolarDate};

/// Returns the holiday observed on its actual calendar date.
///
/// Chinese lunar holidays take precedence over fixed Gregorian holidays when
/// both occur on the same day.
pub fn holiday_for(date: NaiveDate) -> Option<&'static str> {
    let solar = SolarDate {
        year: date.year(),
        month: date.month() as u8,
        day: date.day() as u8,
    };

    if let Ok(lunar) = solar_to_lunar(solar) {
        if !lunar.is_leap_month {
            match (lunar.month, lunar.day) {
                (1, 1) => return Some("春节"),
                (1, 15) => return Some("元宵"),
                (5, 5) => return Some("端午"),
                (8, 15) => return Some("中秋"),
                _ => {}
            }
        }
    }

    match (date.month(), date.day()) {
        (1, 1) => Some("元旦"),
        (5, 1) => Some("劳动节"),
        (10, 1) => Some("国庆节"),
        (12, 25) => Some("Christmas"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::holiday_for;
    use chrono::NaiveDate;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn names_spring_festival() {
        assert_eq!(holiday_for(date(2024, 2, 10)), Some("春节"));
    }

    #[test]
    fn names_lantern_festival() {
        assert_eq!(holiday_for(date(2024, 2, 24)), Some("元宵"));
    }

    #[test]
    fn names_dragon_boat_festival() {
        assert_eq!(holiday_for(date(2024, 6, 10)), Some("端午"));
    }

    #[test]
    fn names_mid_autumn_festival() {
        assert_eq!(holiday_for(date(2024, 9, 17)), Some("中秋"));
    }

    #[test]
    fn names_fixed_gregorian_holidays() {
        assert_eq!(holiday_for(date(2024, 1, 1)), Some("元旦"));
        assert_eq!(holiday_for(date(2024, 5, 1)), Some("劳动节"));
        assert_eq!(holiday_for(date(2024, 10, 1)), Some("国庆节"));
        assert_eq!(holiday_for(date(2024, 12, 25)), Some("Christmas"));
    }

    #[test]
    fn chinese_holiday_wins_on_collision() {
        assert_eq!(holiday_for(date(2020, 10, 1)), Some("中秋"));
    }

    #[test]
    fn returns_none_for_an_ordinary_date() {
        assert_eq!(holiday_for(date(2024, 3, 12)), None);
    }
}
