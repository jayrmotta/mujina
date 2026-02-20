use std::collections::VecDeque;

/// A sliding window filter for temperature readings that rejects noise.
///
/// Maintains a moving average of recent readings and rejects readings that
/// deviate too much from this average, which helps filter out sensor noise
/// while still responding to genuine temperature changes.
#[derive(Debug, Clone)]
pub struct TemperatureFilter {
    window: VecDeque<f32>,
    window_size: u8,
    max_deviation_c: f32,
}

impl TemperatureFilter {
    /// Creates a new temperature filter with the specified window size and maximum deviation.
    ///
    /// # Arguments
    /// * `window_size` - Number of recent readings to maintain in the sliding window
    /// * `max_deviation_c` - Maximum allowed deviation from the moving average (in °C)
    pub fn new(window_size: u8, max_deviation_c: f32) -> Self {
        Self {
            window: VecDeque::with_capacity(window_size as usize),
            window_size,
            max_deviation_c,
        }
    }

    /// Considers a new reading; returns `Some(temp)` if accepted, `None` if rejected as noise.
    ///
    /// Readings are rejected if:
    /// - They are outside the valid range (-20°C to 100°C)
    /// - They deviate more than `max_deviation_c` from the current moving average
    ///
    /// Valid readings are added to the sliding window and returned.
    pub fn consider(&mut self, temp: f32) -> Option<f32> {
        if !(-20.0..=100.0).contains(&temp) {
            return None;
        }

        if !self.window.is_empty() {
            let avg = self.window.iter().sum::<f32>() / self.window.len() as f32;
            let deviation = (temp - avg).abs();
            if deviation > self.max_deviation_c {
                return None;
            }
        }

        if self.window.len() == self.window_size as usize {
            self.window.pop_front();
        }

        self.window.push_back(temp);

        Some(temp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_accept_valid_reading_when_window_is_empty() {
        let mut filter = TemperatureFilter::new(5, 5.0);

        let result = filter.consider(45.0);

        assert_eq!(result, Some(45.0));
        assert_eq!(filter.window.len(), 1);
    }

    #[test]
    fn should_reject_reading_below_valid_range() {
        let mut filter = TemperatureFilter::new(5, 5.0);

        let result = filter.consider(-21.0);

        assert_eq!(result, None);
        assert_eq!(filter.window.len(), 0);
    }

    #[test]
    fn should_reject_reading_above_valid_range() {
        let mut filter = TemperatureFilter::new(5, 5.0);

        let result = filter.consider(101.0);

        assert_eq!(result, None);
        assert_eq!(filter.window.len(), 0);
    }

    #[test]
    fn should_accept_reading_at_lower_bound() {
        let mut filter = TemperatureFilter::new(5, 5.0);

        let result = filter.consider(-20.0);

        assert_eq!(result, Some(-20.0));
    }

    #[test]
    fn should_accept_reading_at_upper_bound() {
        let mut filter = TemperatureFilter::new(5, 5.0);

        let result = filter.consider(100.0);

        assert_eq!(result, Some(100.0));
    }

    #[test]
    fn should_reject_reading_that_exceeds_max_deviation() {
        let mut filter = TemperatureFilter::new(5, 5.0);

        filter.consider(50.0);
        filter.consider(51.0);
        filter.consider(52.0);

        let result = filter.consider(65.0);

        assert_eq!(result, None);
    }

    #[test]
    fn should_accept_reading_within_max_deviation() {
        let mut filter = TemperatureFilter::new(5, 5.0);

        filter.consider(50.0);
        filter.consider(51.0);
        filter.consider(52.0);

        let result = filter.consider(54.0);

        assert_eq!(result, Some(54.0));
    }

    #[test]
    fn should_maintain_sliding_window_size() {
        let mut filter = TemperatureFilter::new(3, 5.0);

        filter.consider(50.0);
        filter.consider(51.0);
        filter.consider(52.0);
        filter.consider(53.0);

        assert_eq!(filter.window.len(), 3);
        assert_eq!(*filter.window.front().unwrap(), 51.0);
    }

    #[test]
    fn should_accept_readings_after_noise_rejection() {
        let mut filter = TemperatureFilter::new(5, 5.0);

        filter.consider(50.0);
        filter.consider(51.0);
        filter.consider(52.0);
        filter.consider(65.0); // Rejected

        let result = filter.consider(53.0);

        assert_eq!(result, Some(53.0));
    }

    #[test]
    fn should_have_deterministic_behavior_with_known_values() {
        let mut filter = TemperatureFilter::new(4, 2.0);

        assert_eq!(filter.consider(50.0), Some(50.0));
        assert_eq!(filter.consider(51.0), Some(51.0));
        assert_eq!(filter.consider(50.5), Some(50.5));
        assert_eq!(filter.consider(51.2), Some(51.2));
        assert_eq!(filter.consider(58.0), None); // Too much deviation
        assert_eq!(filter.consider(52.0), Some(52.0));
    }
}
