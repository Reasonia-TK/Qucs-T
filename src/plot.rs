//! Number formatting shared by the result diagrams and the GUI.

/// Engineering notation with SI prefix: 1.5e-9 -> "1.5n".
pub fn eng(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    let (d, s) = [(1e12, "T"), (1e9, "G"), (1e6, "M"), (1e3, "k"), (1.0, ""), (1e-3, "m"), (1e-6, "µ"), (1e-9, "n"), (1e-12, "p"), (1e-15, "f")]
        .iter()
        .find(|(d, _)| a >= *d * 0.999)
        .copied()
        .unwrap_or((1e-15, "f"));
    let x = v / d;
    let t = format!("{:.3}", x);
    let t = t.trim_end_matches('0').trim_end_matches('.').to_string();
    format!("{t}{s}")
}

/// Interpolate a saved signal on its time axis. At an event, use the post-event sample.
pub fn sample_at(time: &[f64], values: &[f64], at: f64) -> Option<f64> {
    if !(*time.first()?..=*time.last()?).contains(&at) {
        return None;
    }
    let right = time.partition_point(|&t| t <= at);
    let left = right - 1;
    let value = values[left];
    Some(if let Some(&next) = time.get(right) {
        let last = time[left];
        value + (values[right] - value) * ((at - last) / (next - last))
    } else {
        value
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_in_range_and_uses_right_side_of_discontinuities() {
        let time = [0.0, 1.0, 1.0, 2.0];
        let values = [25.0, 27.0, 35.0, 39.0];
        assert_eq!(sample_at(&time, &values, 0.5), Some(26.0));
        assert_eq!(sample_at(&time, &values, 1.0), Some(35.0));
        assert_eq!(sample_at(&time, &values, 1.5), Some(37.0));
        assert_eq!(sample_at(&time, &values, 2.1), None);
    }
}

