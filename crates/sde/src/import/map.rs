//! The map readers of the download: regions, solar systems and stargates, checked against each other.

use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Number;

use crate::error::SdeError;

use super::{
    LocalizedName, MAP_REGIONS_FILE, MAP_STARGATES_FILE, MAP_SYSTEMS_FILE, Source, bad_reference,
    parse_file, read_rows, required,
};

/// The regions, solar systems and stargates of the download, checked against each other.
pub(super) struct MapData {
    pub(super) regions: Vec<RegionRow>,
    pub(super) systems: Vec<SystemRow>,
    pub(super) gates: Vec<GateRow>,
}

pub(super) struct RegionRow {
    pub(super) id: u32,
    pub(super) name: String,
}

pub(super) struct SystemRow {
    pub(super) id: u32,
    pub(super) name: String,
    pub(super) region_id: u32,
    /// Security status in millionths, as the text of the download gives it.
    pub(super) security_millionths: i32,
}

pub(super) struct GateRow {
    pub(super) id: u32,
    pub(super) from_system: u32,
    pub(super) to_system: u32,
}

#[derive(Deserialize)]
struct RegionRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    name: Option<LocalizedName>,
}

#[derive(Deserialize)]
struct SystemRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    name: Option<LocalizedName>,
    #[serde(rename = "regionID")]
    region_id: Option<u32>,
    #[serde(rename = "securityStatus")]
    security_status: Option<Number>,
}

#[derive(Deserialize)]
struct StargateRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    #[serde(rename = "solarSystemID")]
    system_id: Option<u32>,
    destination: Option<StargateEnd>,
}

#[derive(Deserialize)]
struct StargateEnd {
    #[serde(rename = "solarSystemID")]
    system_id: Option<u32>,
}

impl MapData {
    /// Reads the regions, then the systems, then the stargates. A system must be in a listed region,
    /// and both ends of a stargate must be listed systems.
    pub(super) fn read(source: &mut Source) -> Result<Self, SdeError> {
        let regions = read_rows(source, MAP_REGIONS_FILE, RegionRow::from_record)?;
        let region_ids: BTreeSet<u32> = regions.iter().map(|row| row.id).collect();
        let systems = parse_file::<SystemRecord>(source, MAP_SYSTEMS_FILE)?
            .into_iter()
            .map(|(line, record)| SystemRow::from_record(record, line, &region_ids))
            .collect::<Result<Vec<_>, _>>()?;
        let system_ids: BTreeSet<u32> = systems.iter().map(|row| row.id).collect();
        let gates = parse_file::<StargateRecord>(source, MAP_STARGATES_FILE)?
            .into_iter()
            .map(|(line, record)| GateRow::from_record(record, line, &system_ids))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            regions,
            systems,
            gates,
        })
    }
}

impl RegionRow {
    fn from_record(record: RegionRecord, line: usize) -> Result<Self, SdeError> {
        Ok(Self {
            id: required(record.key, MAP_REGIONS_FILE, line, "_key")?,
            name: english_name(record.name, MAP_REGIONS_FILE, line)?,
        })
    }
}

impl SystemRow {
    fn from_record(
        record: SystemRecord,
        line: usize,
        region_ids: &BTreeSet<u32>,
    ) -> Result<Self, SdeError> {
        let file = MAP_SYSTEMS_FILE;
        let id = required(record.key, file, line, "_key")?;
        let name = english_name(record.name, file, line)?;
        let region_id = required(record.region_id, file, line, "regionID")?;
        if !region_ids.contains(&region_id) {
            return Err(bad_reference(file, line, "regionID", region_id));
        }
        let text = required(record.security_status, file, line, "securityStatus")?.to_string();
        let security_millionths =
            security_millionths(&text).ok_or_else(|| SdeError::BadNumber {
                file: file.to_owned(),
                line,
                field: "securityStatus".to_owned(),
                text,
            })?;
        Ok(Self {
            id,
            name,
            region_id,
            security_millionths,
        })
    }
}

impl GateRow {
    fn from_record(
        record: StargateRecord,
        line: usize,
        system_ids: &BTreeSet<u32>,
    ) -> Result<Self, SdeError> {
        let file = MAP_STARGATES_FILE;
        let row = Self {
            id: required(record.key, file, line, "_key")?,
            from_system: required(record.system_id, file, line, "solarSystemID")?,
            to_system: required(
                record.destination.and_then(|end| end.system_id),
                file,
                line,
                "destination.solarSystemID",
            )?,
        };
        for (field, system) in [
            ("solarSystemID", row.from_system),
            ("destination.solarSystemID", row.to_system),
        ] {
            if !system_ids.contains(&system) {
                return Err(bad_reference(file, line, field, system));
            }
        }
        Ok(row)
    }
}

pub(crate) fn english_name(
    name: Option<LocalizedName>,
    file: &str,
    line: usize,
) -> Result<String, SdeError> {
    name.and_then(|name| name.en)
        .ok_or_else(|| SdeError::MissingEnglishName {
            file: file.to_owned(),
            line,
        })
}

/// A security status as millionths, from its text: `0.945913` is 945,913 and `-9e-05` is -90.
/// `None` when a non-zero digit lies past the sixth decimal place, when the value is outside
/// -1,000,000..=1,000,000, or when the text is not a number. Trailing zeros are fine.
fn security_millionths(text: &str) -> Option<i32> {
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (text, 0),
    };
    let (negative, unsigned) = match mantissa.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, mantissa),
    };
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if whole.is_empty() || !digits(whole) || !digits(fraction) {
        return None;
    }
    // Trailing zeros of the fraction add no value, so they are dropped before the digits are counted.
    let fraction = fraction.trim_end_matches('0');
    // The value is all the digits times 10^(exponent - fraction length); times 10^6 gives millionths.
    let scale = exponent
        .checked_sub(i32::try_from(fraction.len()).ok()?)?
        .checked_add(6)?;
    let all_digits: i128 = format!("{whole}{fraction}").parse().ok()?;
    let power = 10_i128.checked_pow(scale.unsigned_abs())?;
    let millionths = if scale >= 0 {
        all_digits.checked_mul(power)?
    } else if all_digits % power == 0 {
        all_digits / power
    } else {
        return None;
    };
    let signed = if negative {
        millionths.checked_neg()?
    } else {
        millionths
    };
    if !(-1_000_000..=1_000_000).contains(&signed) {
        return None;
    }
    i32::try_from(signed).ok()
}

#[cfg(test)]
mod tests {
    use super::security_millionths;

    #[test]
    fn rejects_an_empty_whole_part() {
        assert_eq!(security_millionths(".5"), None);
    }

    #[test]
    fn rejects_a_sign_inside_the_mantissa() {
        assert_eq!(security_millionths("+0"), None);
    }

    #[test]
    fn divides_only_when_the_value_is_an_exact_multiple() {
        assert_eq!(security_millionths("100000e-10"), Some(10));
    }

    #[test]
    fn reads_plain_and_exponent_values() {
        assert_eq!(security_millionths("0.945913"), Some(945_913));
        assert_eq!(security_millionths("-9e-05"), Some(-90));
    }
}
