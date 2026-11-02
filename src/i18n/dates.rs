//! Dates in words, in the current language: names of days and months, and each language's order.

use super::{Lang, current};
use jiff::civil::Date;

const MONTHS: [[&str; 12]; 4] = [
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],
    [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ],
    [
        "enero",
        "febrero",
        "marzo",
        "abril",
        "mayo",
        "junio",
        "julio",
        "agosto",
        "septiembre",
        "octubre",
        "noviembre",
        "diciembre",
    ],
    [
        "Januar",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ],
];

const MONTHS_SHORT: [[&str; 12]; 4] = [
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ],
    [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic",
    ],
    [
        "Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.",
        "Dez.",
    ],
];

/// Monday first.
const WEEKDAYS: [[&str; 7]; 4] = [
    [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ],
    [
        "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
    ],
    [
        "lunes",
        "martes",
        "miércoles",
        "jueves",
        "viernes",
        "sábado",
        "domingo",
    ],
    [
        "Montag",
        "Dienstag",
        "Mittwoch",
        "Donnerstag",
        "Freitag",
        "Samstag",
        "Sonntag",
    ],
];

fn index(lang: Lang) -> usize {
    Lang::ALL.iter().position(|l| *l == lang).unwrap_or(0)
}

fn month(names: &[[&'static str; 12]; 4], lang: Lang, day: Date) -> &'static str {
    names[index(lang)][usize::try_from(day.month() - 1).unwrap_or(0)]
}

/// `1 November 2026`, `1 novembre 2026`, `1 de noviembre de 2026`, `1. November 2026`.
pub fn day_month_year(day: Date) -> String {
    let lang = current();
    let (d, m, y) = (day.day(), month(&MONTHS, lang, day), day.year());
    match lang {
        Lang::Es => format!("{d} de {m} de {y}"),
        Lang::De => format!("{d}. {m} {y}"),
        _ => format!("{d} {m} {y}"),
    }
}

/// `Sunday 1 November`, `dimanche 1 novembre`, `domingo 1 de noviembre`, `Sonntag, 1. November`.
pub fn weekday_day_month(day: Date) -> String {
    let lang = current();
    let weekday =
        WEEKDAYS[index(lang)][usize::from(day.weekday().to_monday_zero_offset().unsigned_abs())];
    let (d, m) = (day.day(), month(&MONTHS, lang, day));
    match lang {
        Lang::Es => format!("{weekday} {d} de {m}"),
        Lang::De => format!("{weekday}, {d}. {m}"),
        _ => format!("{weekday} {d} {m}"),
    }
}

/// `1 Nov`, `1 nov.`, `1 nov`, `1. Nov.`: under the daily bars.
pub fn day_short_month(day: Date) -> String {
    let lang = current();
    let (d, m) = (day.day(), month(&MONTHS_SHORT, lang, day));
    match lang {
        Lang::De => format!("{d}. {m}"),
        _ => format!("{d} {m}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::sync_scope;
    use jiff::civil::date;

    #[test]
    fn each_language_names_and_orders_the_date() {
        let day = date(2026, 11, 1);
        let all = |f: fn(Date) -> String| Lang::ALL.map(|lang| sync_scope(lang, || f(day)));
        assert_eq!(
            all(day_month_year),
            [
                "1 November 2026",
                "1 novembre 2026",
                "1 de noviembre de 2026",
                "1. November 2026"
            ]
        );
        assert_eq!(
            all(weekday_day_month),
            [
                "Sunday 1 November",
                "dimanche 1 novembre",
                "domingo 1 de noviembre",
                "Sonntag, 1. November"
            ]
        );
        assert_eq!(
            all(day_short_month),
            ["1 Nov", "1 nov.", "1 nov", "1. Nov."]
        );
        assert_eq!(day_short_month(date(2026, 3, 9)), "9 Mar");
    }
}
