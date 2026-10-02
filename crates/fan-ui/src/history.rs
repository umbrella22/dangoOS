//! Keep Slint rows alive as the temperature window advances.

use alloc::{rc::Rc, vec::Vec};
use slint::{Model, ModelRc, VecModel};

use crate::TemperatureSample;

pub const HISTORY_CAPACITY: usize = 30;

pub struct TemperatureHistory {
    values: [f32; HISTORY_CAPACITY],
    len: usize,
    model: Rc<VecModel<TemperatureSample>>,
}

impl Default for TemperatureHistory {
    fn default() -> Self {
        Self {
            values: [0.0; HISTORY_CAPACITY],
            len: 0,
            model: Rc::new(VecModel::from(Vec::with_capacity(HISTORY_CAPACITY))),
        }
    }
}

impl TemperatureHistory {
    pub fn model(&self) -> ModelRc<TemperatureSample> {
        self.model.clone().into()
    }

    pub fn push(&mut self, value: f32) {
        if self.len == HISTORY_CAPACITY {
            self.values.copy_within(1.., 0);
        } else {
            self.len += 1;
        }
        self.values[self.len - 1] = value;
        self.update_rows();
    }

    /// Display the most recent samples from an existing history, oldest first.
    pub fn set_samples(&mut self, values: &[f32]) {
        let values = &values[values.len().saturating_sub(HISTORY_CAPACITY)..];
        self.len = values.len();
        self.values[..self.len].copy_from_slice(values);
        self.update_rows();
    }

    fn update_rows(&self) {
        for i in 0..self.len {
            // Each segment owns both endpoints. Reading the next row from a
            // Slint binding would subscribe every segment to model-wide changes.
            let row = TemperatureSample {
                value: self.values[i],
                next_value: self.values[(i + 1).min(self.len - 1)],
            };
            match self.model.row_data(i) {
                Some(old) if old == row => {}
                Some(_) => self.model.set_row_data(i, row),
                None => self.model.push(row),
            }
        }
        // Only an explicit shorter replacement removes rows. Rolling samples
        // keep the model and all existing repeater instances alive.
        while self.model.row_count() > self.len {
            self.model.remove(self.model.row_count() - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_history_keeps_the_latest_values_and_adjacent_endpoints() {
        let mut history = TemperatureHistory::default();
        let model = history.model();
        for value in 0..100 {
            history.push(value as f32);
            let count = (value + 1).min(HISTORY_CAPACITY);
            assert_eq!(model.row_count(), count);
            for row in 0..count {
                let sample = model.row_data(row).unwrap();
                assert_eq!(sample.value, (value + 1 - count + row) as f32);
                assert_eq!(
                    sample.next_value,
                    (value + 2 - count + row).min(value) as f32
                );
            }
        }
    }

    #[test]
    fn replacing_history_handles_short_empty_and_long_windows() {
        let mut history = TemperatureHistory::default();
        let values: Vec<f32> = (0..50).map(|v| v as f32).collect();
        history.set_samples(&values);
        assert_eq!(history.model.row_count(), HISTORY_CAPACITY);
        assert_eq!(history.model.row_data(0).unwrap().value, 20.0);
        history.set_samples(&[42.0]);
        assert_eq!(history.model.row_count(), 1);
        assert_eq!(history.model.row_data(0).unwrap().next_value, 42.0);
        history.set_samples(&[]);
        assert_eq!(history.model.row_count(), 0);
        history.push(25.0);
        assert_eq!(history.model.row_data(0).unwrap().value, 25.0);
    }
}
