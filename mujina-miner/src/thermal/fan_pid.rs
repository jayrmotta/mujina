use std::time::Duration;

#[derive(Debug, Clone)]
pub struct FanPIDController {
    pub proportional_gain: f32,
    pub integral_sum: f32,
    pub integral_gain: f32,
    pub integral_min: f32,
    pub integral_max: f32,
}

impl FanPIDController {
    pub fn new(
        proportional_gain: f32,
        integral_sum: f32,
        integral_gain: f32,
        integral_min: f32,
        integral_max: f32,
    ) -> Self {
        Self {
            proportional_gain,
            integral_sum,
            integral_gain,
            integral_min,
            integral_max,
        }
    }

    pub fn update(
        &mut self,
        error: f32,
        time_since_last_update: Duration,
        freeze_integral: bool,
    ) -> f32 {
        let dt_s = time_since_last_update.as_secs_f32();

        if !freeze_integral {
            self.integral_sum += error * dt_s;
            self.integral_sum = self
                .integral_sum
                .clamp(self.integral_min, self.integral_max);
        }

        self.proportional_gain * error + self.integral_gain * self.integral_sum
    }

    pub fn reset(&mut self) {
        self.integral_sum = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_combine_proportional_and_integral_terms() {
        let mut controller = FanPIDController::new(1.0, 0.5, 2.0, -10.0, 10.0);

        controller.integral_sum = 3.0;
        let output = controller.update(5.0, Duration::from_secs(1), true);
        assert_eq!(output, 11.0);
    }

    #[test]
    fn should_accumulate_integral_when_freeze_integral_is_false() {
        let mut controller = FanPIDController::new(1.0, 0.5, 0.0, -10.0, 10.0);

        let initial_integral_sum = controller.integral_sum;
        controller.update(2.0, Duration::from_secs(1), false);
        assert_eq!(controller.integral_sum, initial_integral_sum + 2.0);
    }

    #[test]
    fn should_not_accumulate_integral_when_freeze_integral_is_true() {
        let mut controller = FanPIDController::new(1.0, 0.5, 0.0, -10.0, 10.0);

        let initial_integral_sum = controller.integral_sum;
        controller.update(2.0, Duration::from_secs(1), true);
        assert_eq!(controller.integral_sum, initial_integral_sum);
    }

    #[test]
    fn should_clamp_integral_at_max_boundary() {
        let mut controller = FanPIDController::new(1.0, 0.5, 0.0, -10.0, 10.0);

        controller.integral_sum = 9.0;
        controller.update(5.0, Duration::from_secs(1), false);
        assert_eq!(controller.integral_sum, 10.0);
    }

    #[test]
    fn should_clamp_integral_at_min_boundary() {
        let mut controller = FanPIDController::new(1.0, 0.5, 0.0, -10.0, 10.0);

        controller.integral_sum = -9.0;
        controller.update(-5.0, Duration::from_secs(1), false);
        assert_eq!(controller.integral_sum, -10.0);
    }
}
