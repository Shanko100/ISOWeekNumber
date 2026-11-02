//! Public holidays from the embedded table: the country list for the
//! Holidays dialog, and the years the calendar shows, held only while it is
//! open (and only the years on screen).

use crate::dates::Date;
use crate::pack::{self, MAX_HOLIDAY_YEAR, MIN_HOLIDAY_YEAR};
use std::collections::{HashMap, HashSet};

/// Every country code with holiday data.
pub fn country_codes() -> Vec<String> {
    pack::holiday_index()
        .map(|(list, _)| list.iter().map(|c| c.code_str().to_string()).collect())
        .unwrap_or_default()
}

/// (code, English name), sorted by name ignoring case, as the dialog lists them.
pub fn countries_by_name() -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = pack::holiday_index()
        .map(|(list, _)| {
            list.iter()
                .map(|c| (c.code_str().to_string(), c.name.to_string()))
                .collect()
        })
        .unwrap_or_default();
    list.sort_by_key(|(_, name)| name.to_lowercase());
    list
}

/// The years on screen of the selected countries.
#[derive(Default)]
pub struct Store {
    /// code -> year -> day of year -> names
    tables: HashMap<String, HashMap<i32, HashMap<u16, Vec<String>>>>,
    names: HashMap<String, String>,
    failed: HashSet<String>,
}

impl Store {
    /// Keep exactly `years` of `countries`, loading what is missing.
    pub fn hold(&mut self, countries: &[Option<&str>], years: &[i32]) {
        let wanted: Vec<&str> = countries.iter().flatten().copied().collect();
        let keep: Vec<i32> = years
            .iter()
            .copied()
            .filter(|y| (MIN_HOLIDAY_YEAR..=MAX_HOLIDAY_YEAR).contains(y))
            .collect();
        self.tables
            .retain(|code, _| wanted.contains(&code.as_str()));
        self.names.retain(|code, _| wanted.contains(&code.as_str()));
        for table in self.tables.values_mut() {
            table.retain(|y, _| keep.contains(y));
        }
        let missing: Vec<&str> = wanted
            .iter()
            .copied()
            .filter(|c| !self.failed.contains(*c))
            .filter(|c| {
                self.tables
                    .get(*c)
                    .is_none_or(|t| keep.iter().any(|y| !t.contains_key(y)))
            })
            .collect();
        if missing.is_empty() {
            return;
        }
        let Some((index, chunks)) = pack::holiday_index() else {
            self.failed.extend(missing.iter().map(|c| c.to_string()));
            return;
        };
        for code in missing {
            let Some(entry) = index.iter().find(|e| e.code_str() == code) else {
                self.failed.insert(code.to_string());
                continue;
            };
            match pack::holiday_years(entry, chunks, &keep) {
                Some(years) => {
                    let table = self.tables.entry(code.to_string()).or_default();
                    for (year, days) in years {
                        table.insert(year, days.into_iter().collect());
                    }
                    self.names.insert(code.to_string(), entry.name.to_string());
                }
                None => {
                    self.failed.insert(code.to_string());
                    self.tables.remove(code);
                }
            }
        }
    }

    pub fn names(&self, country: Option<&str>, date: Date) -> &[String] {
        let Some(c) = country else { return &[] };
        let doy = (date.to_days()
            - Date {
                year: date.year,
                month: 1,
                day: 1,
            }
            .to_days()) as u16;
        self.tables
            .get(c)
            .and_then(|t| t.get(&date.year))
            .and_then(|d| d.get(&doy))
            .map_or(&[], Vec::as_slice)
    }

    pub fn country_name(&self, code: &str) -> Option<&str> {
        self.names.get(code).map(String::as_str)
    }
}
