//! Measured temperature CSV data and the numeric series selected for plotting.

use crate::thermal::Series;
use csv::StringRecord;
use std::io::Read;
use std::path::Path;

pub struct MeasurementCsv {
    pub columns: Vec<String>,
    records: Vec<StringRecord>,
}

pub struct Measurement {
    pub time_s: Vec<f64>,
    pub temperature: Series,
    pub missing: usize,
}

impl MeasurementCsv {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("CSV を開けません（{e}）"))?;
        Self::parse(file)
    }

    fn parse(reader: impl Read) -> Result<Self, String> {
        let mut csv = csv::ReaderBuilder::new()
            .has_headers(false)
            .trim(csv::Trim::All)
            .from_reader(reader);
        let mut records = csv.records();
        let read_error =
            |error: csv::Error| format!("UTF-8 のカンマ区切り CSV を読み込めません（{error}）");
        let first = records
            .next()
            .ok_or("CSV にデータがありません")?
            .map_err(read_error)?;
        if first.len() < 2 {
            return Err("CSV には時刻列と温度列が必要です。カンマ区切りで保存してください".into());
        }
        // Numeric first rows belong to the measurements. Text-only first rows are headers.
        let header = first.iter().any(|field| !field.is_empty())
            && first.iter().all(|field| field.parse::<f64>().is_err());
        let columns = first
            .iter()
            .enumerate()
            .map(|(i, label)| {
                if header && !label.is_empty() {
                    label.into()
                } else {
                    format!("列 {}", i + 1)
                }
            })
            .collect();
        let records: Vec<_> = (!header)
            .then_some(Ok(first))
            .into_iter()
            .chain(records)
            .collect::<Result<_, _>>()
            .map_err(read_error)?;
        if records.is_empty() {
            return Err("CSV に測定行がありません".into());
        }
        Ok(Self { columns, records })
    }

    pub fn default_columns(&self) -> (usize, usize) {
        let time = self
            .columns
            .iter()
            .position(|label| {
                label.to_ascii_lowercase().contains("time")
                    || label.contains("時刻")
                    || label.contains("時間")
            })
            .unwrap_or(0);
        let temperature = self
            .columns
            .iter()
            .enumerate()
            .find(|&(i, label)| {
                i != time
                    && (label.to_ascii_lowercase().contains("temp")
                        || label.contains("温度")
                        || label.contains("°C")
                        || label.contains("℃"))
            })
            .map_or(usize::from(time == 0), |(i, _)| i);
        (time, temperature)
    }

    pub fn select(&self, time: usize, temperature: usize) -> Result<Measurement, String> {
        if time == temperature || time >= self.columns.len() || temperature >= self.columns.len() {
            return Err("時刻列と温度列を別々に選択してください".into());
        }
        let number = |field: &str, row, label| {
            field
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("CSV の {row} 行目の{label}を有限の数値で指定してください"))
        };
        let mut time_s = Vec::new();
        let mut values = Vec::new();
        let mut missing = 0;
        for record in &self.records {
            if record[temperature].is_empty() {
                missing += 1;
                continue;
            }
            let row = record.position().map_or(0, csv::Position::line);
            let t = number(&record[time], row, "時刻 [s]")?;
            let temp = number(&record[temperature], row, "温度 [°C]")?;
            if time_s.last().is_some_and(|&last| t < last) {
                return Err(format!(
                    "CSV の {row} 行目の時刻が前の測定より早くなっています"
                ));
            }
            time_s.push(t);
            values.push(temp);
        }
        if values.is_empty() {
            return Err("選んだ温度列に数値がありません".into());
        }
        Ok(Measurement {
            time_s,
            temperature: Series {
                name: self.columns[temperature].clone(),
                index: temperature,
                values,
            },
            missing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_utf8_bom_quoted_headers_and_selects_temperature_columns() {
        let csv = MeasurementCsv::parse("\u{feff}\"温度, 点A\",時刻 [s],温度B,メモ\r\n24,-1,28,開始\r\n,0,30,欠測\r\n26,0,31,再開\r\n".as_bytes()).unwrap();
        assert_eq!(csv.default_columns(), (1, 0));
        let a = csv.select(1, 0).unwrap();
        assert_eq!(a.temperature.name, "温度, 点A");
        assert_eq!(a.temperature.values, [24.0, 26.0]);
        assert_eq!(a.time_s, [-1.0, 0.0]);
        assert_eq!(a.missing, 1);
        let b = csv.select(1, 2).unwrap();
        assert_eq!(b.time_s, [-1.0, 0.0, 0.0]);
        assert_eq!(b.temperature.values, [28.0, 30.0, 31.0]);
    }

    #[test]
    fn checks_numeric_rows_and_accepts_headerless_measurements() {
        let table = MeasurementCsv::parse(&b"0,25\n1,30\n"[..]).unwrap();
        assert_eq!(table.select(0, 1).unwrap().temperature.values, [25.0, 30.0]);
        assert!(table.select(0, 0).is_err());
        for input in ["0,NaN\n1,30\n", "1,30\n0,20\n", "0,invalid\n1,30\n"] {
            let error = MeasurementCsv::parse(input.as_bytes())
                .unwrap()
                .select(0, 1)
                .err()
                .unwrap();
            assert!(error.contains("行目"), "{error}");
        }
    }
}
