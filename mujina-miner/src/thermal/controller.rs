use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::config::ThermalConfig;
use super::fan_pid::FanPIDController;
use super::state::ThermalState;
use crate::tracing::prelude::*;

const FAN_SPEED_NORMAL: u8 = 30;
const FAN_SPEED_COOLING: u8 = 50;
const FAN_SPEED_THROTTLING: u8 = 80;
const FAN_SPEED_CRITICAL: u8 = 100;

const FAN_SPEED_MIN: f32 = 0.0;
const FAN_SPEED_MAX: f32 = 100.0;

const TICK_DURATION: Duration = Duration::from_secs(5);

/// Cooldown between frequency adjustments to let thermal changes settle.
const FREQUENCY_ADJUSTMENT_INTERVAL: Duration = Duration::from_secs(20);

/// Deadband above target before frequency reduction triggers, to avoid
/// reacting to transient spikes.
const FREQUENCY_OVER_TARGET_MARGIN_C: f32 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FanSpeedCommand {
    pub speed_percent: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrequencyCommand {
    BumpUp,
    BumpDown,
}

pub struct ThermalController {
    config: ThermalConfig,
    tick_duration: Duration,
    fan_pid: FanPIDController,
    fan_speed_tx: watch::Sender<FanSpeedCommand>,
    frequency_tx: mpsc::Sender<FrequencyCommand>,
    temperature_rx: watch::Receiver<Option<f32>>,
    current_state: ThermalState,
    last_tick_time: Option<Instant>,
    last_frequency_adjust: Option<Instant>,
}

impl ThermalController {
    pub fn new(
        config: ThermalConfig,
        fan_pid: FanPIDController,
        fan_speed_tx: watch::Sender<FanSpeedCommand>,
        frequency_tx: mpsc::Sender<FrequencyCommand>,
        temperature_rx: watch::Receiver<Option<f32>>,
    ) -> Self {
        Self {
            config,
            tick_duration: TICK_DURATION,
            fan_pid,
            fan_speed_tx,
            frequency_tx,
            temperature_rx,
            current_state: ThermalState::NORMAL,
            last_tick_time: None,
            last_frequency_adjust: None,
        }
    }

    pub async fn run(mut self, cancellation: CancellationToken) {
        let mut interval = tokio::time::interval(self.tick_duration);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancellation.cancelled() => {
                    break;
                }
                _ = interval.tick() => {
                    self.tick().await;
                }
            }
        }
    }

    pub fn current_state(&self) -> ThermalState {
        self.current_state
    }

    pub fn fan_pid_integral_sum(&self) -> f32 {
        self.fan_pid.integral_sum
    }

    pub fn fan_pid_integral_gain(&self) -> f32 {
        self.fan_pid.integral_gain
    }

    #[cfg(test)]
    fn set_state(&mut self, state: ThermalState) {
        self.current_state = state;
    }

    #[cfg(test)]
    fn set_fan_pid_integral_sum(&mut self, value: f32) {
        self.fan_pid.integral_sum = value;
    }

    #[cfg(test)]
    fn set_tick_duration(&mut self, duration: Duration) {
        self.tick_duration = duration;
    }

    async fn tick(&mut self) {
        let temperature = match *self.temperature_rx.borrow() {
            Some(temp) => temp,
            None => {
                debug!("Thermal controller tick: no temperature reading available yet");
                return;
            }
        };

        let now = Instant::now();
        let time_since_last_tick = self
            .last_tick_time
            .map(|last| now.duration_since(last))
            .unwrap_or(self.tick_duration);
        self.last_tick_time = Some(now);

        let new_state =
            ThermalState::from_temperature(temperature, self.current_state, &self.config);
        let previous_state = self.current_state;
        let state_changed = new_state != self.current_state;

        if state_changed {
            info!(
                previous_state = ?previous_state,
                new_state = ?new_state,
                "Thermal state changed"
            );
        }

        self.current_state = new_state;

        self.adjust_fan_speed(temperature, time_since_last_tick, state_changed)
            .await;
        self.adjust_frequency(previous_state, state_changed, temperature, now)
            .await;
    }

    async fn adjust_fan_speed(
        &mut self,
        temperature: f32,
        time_since_last_tick: Duration,
        state_changed: bool,
    ) {
        let target_temperature = self.config.target_temperature_c;
        let error = temperature - target_temperature;

        let freeze_integral = matches!(
            self.current_state,
            ThermalState::NORMAL | ThermalState::CRITICAL
        );

        let pid_output = self
            .fan_pid
            .update(error, time_since_last_tick, freeze_integral);

        if state_changed && matches!(self.current_state, ThermalState::NORMAL) {
            self.fan_pid.reset();
            debug!("Fan PID reset on transition to NORMAL state");
        }

        let base_speed = match self.current_state {
            ThermalState::NORMAL => FAN_SPEED_NORMAL,
            ThermalState::COOLING => FAN_SPEED_COOLING,
            ThermalState::THROTTLING => FAN_SPEED_THROTTLING,
            ThermalState::CRITICAL => FAN_SPEED_CRITICAL,
        };

        let speed = (base_speed as f32 + pid_output).clamp(FAN_SPEED_MIN, FAN_SPEED_MAX) as u8;

        debug!(
            temp_c = %temperature,
            state = ?self.current_state,
            target_c = %target_temperature,
            error_c = %error,
            pid_output = %pid_output,
            base_speed_pct = %base_speed,
            final_speed_pct = %speed,
            integral_sum = %self.fan_pid.integral_sum,
            freeze_integral = freeze_integral,
            "Thermal control tick"
        );

        if self
            .fan_speed_tx
            .send(FanSpeedCommand {
                speed_percent: speed,
            })
            .is_err()
        {
            debug!("Fan speed command channel closed");
        }
    }

    async fn adjust_frequency(
        &mut self,
        previous_state: ThermalState,
        state_changed: bool,
        temperature: f32,
        now: Instant,
    ) {
        if !state_changed {
            let over_target =
                temperature >= self.config.target_temperature_c + FREQUENCY_OVER_TARGET_MARGIN_C;
            let throttling = matches!(
                self.current_state,
                ThermalState::THROTTLING | ThermalState::CRITICAL
            );

            let cooldown_active = self
                .last_frequency_adjust
                .map(|last| now.duration_since(last) < FREQUENCY_ADJUSTMENT_INTERVAL)
                .unwrap_or(false);

            if throttling && over_target && !cooldown_active {
                let cmd = FrequencyCommand::BumpDown;
                info!(
                    state = ?self.current_state,
                    temp_c = %temperature,
                    target_c = %self.config.target_temperature_c,
                    margin_c = %FREQUENCY_OVER_TARGET_MARGIN_C,
                    command = ?cmd,
                    "Thermal frequency adjustment (sustained overshoot)"
                );
                if self.frequency_tx.send(cmd).await.is_err() {
                    debug!(command = ?cmd, "Frequency command channel closed");
                }
                self.last_frequency_adjust = Some(now);
            }
            return;
        }

        fn state_severity(state: ThermalState) -> u8 {
            match state {
                ThermalState::NORMAL => 0,
                ThermalState::COOLING => 1,
                ThermalState::THROTTLING => 2,
                ThermalState::CRITICAL => 3,
            }
        }

        let previous_severity = state_severity(previous_state);
        let current_severity = state_severity(self.current_state);

        let cmd = if current_severity > previous_severity {
            FrequencyCommand::BumpDown
        } else {
            FrequencyCommand::BumpUp
        };

        info!(
            previous_state = ?previous_state,
            new_state = ?self.current_state,
            command = ?cmd,
            "Thermal frequency adjustment"
        );

        if self.frequency_tx.send(cmd).await.is_err() {
            debug!(command = ?cmd, "Frequency command channel closed");
        }

        self.last_frequency_adjust = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::super::state::{HYSTERESIS_C, NORMAL_THRESHOLD_C};
    use super::*;
    use tokio::sync::{mpsc, watch};

    fn create_controller() -> (
        ThermalController,
        watch::Receiver<FanSpeedCommand>,
        mpsc::Receiver<FrequencyCommand>,
        watch::Sender<Option<f32>>,
    ) {
        let (fan_tx, fan_rx) = watch::channel(FanSpeedCommand { speed_percent: 0 });
        let (freq_tx, freq_rx) = mpsc::channel(3);
        let (temp_tx, temp_rx) = watch::channel(None::<f32>);

        let config = ThermalConfig::default();
        let fan_pid = FanPIDController::new(1.0, 0.1, 0.0, -10.0, 10.0);

        let controller = ThermalController::new(config, fan_pid, fan_tx, freq_tx, temp_rx);

        (controller, fan_rx, freq_rx, temp_tx)
    }

    #[tokio::test]
    async fn should_send_fan_speed_command_on_tick() {
        let (mut controller, fan_rx, _freq_rx, temp_tx) = create_controller();
        let config = ThermalConfig::default();

        temp_tx
            .send(Some(config.target_temperature_c + 1.0))
            .unwrap();
        controller.tick().await;

        let command = *fan_rx.borrow();
        assert_eq!(command.speed_percent, 51);
    }

    #[tokio::test]
    async fn should_not_send_commands_when_no_temperature_received() {
        let (mut controller, fan_rx, mut freq_rx, _temp_tx) = create_controller();

        controller.tick().await;

        assert_eq!(fan_rx.borrow().speed_percent, 0);
        assert!(freq_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn should_send_frequency_command_on_state_change() {
        let (mut controller, _fan_rx, mut freq_rx, temp_tx) = create_controller();

        temp_tx.send(Some(75.0)).unwrap();
        controller.tick().await;

        let command = freq_rx.try_recv().unwrap();
        assert_eq!(command, FrequencyCommand::BumpDown);
    }

    #[tokio::test]
    async fn should_not_send_frequency_command_when_state_unchanged() {
        let (mut controller, _fan_rx, mut freq_rx, temp_tx) = create_controller();

        temp_tx.send(Some(NORMAL_THRESHOLD_C + 1.0)).unwrap();
        controller.tick().await;

        freq_rx.try_recv().unwrap();

        temp_tx.send(Some(NORMAL_THRESHOLD_C + 2.0)).unwrap();
        controller.tick().await;

        assert!(freq_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn should_reset_fan_pid_on_transition_to_normal() {
        let (mut controller, _fan_rx, _freq_rx, temp_tx) = create_controller();

        temp_tx.send(Some(65.0)).unwrap();
        controller.tick().await;

        controller.set_fan_pid_integral_sum(5.0);

        temp_tx.send(Some(45.0)).unwrap();
        controller.tick().await;

        assert_eq!(controller.fan_pid_integral_sum(), 0.0);
    }

    #[tokio::test]
    async fn should_set_fan_speed_based_on_thermal_state() {
        let (mut controller, fan_rx, _freq_rx, temp_tx) = create_controller();
        let config = ThermalConfig::default();

        temp_tx.send(Some(NORMAL_THRESHOLD_C - 1.0)).unwrap();
        controller.tick().await;
        let normal_cmd = *fan_rx.borrow();
        assert_eq!(normal_cmd.speed_percent, 10);

        temp_tx.send(Some(NORMAL_THRESHOLD_C + 1.0)).unwrap();
        controller.tick().await;
        let cooling_cmd = *fan_rx.borrow();
        assert_eq!(cooling_cmd.speed_percent, 32);

        temp_tx
            .send(Some(config.target_temperature_c + 1.0))
            .unwrap();
        controller.tick().await;
        let throttling_cmd = *fan_rx.borrow();
        assert_eq!(throttling_cmd.speed_percent, 81);

        temp_tx.send(Some(config.max_temperature_c + 1.0)).unwrap();
        controller.tick().await;
        let critical_cmd = *fan_rx.borrow();
        assert_eq!(critical_cmd.speed_percent, 100);
    }

    #[tokio::test]
    async fn should_send_bump_down_when_state_becomes_more_severe() {
        let (mut controller, _fan_rx, mut freq_rx, temp_tx) = create_controller();
        let config = ThermalConfig::default();

        controller.set_state(ThermalState::NORMAL);
        temp_tx.send(Some(NORMAL_THRESHOLD_C + 1.0)).unwrap();
        controller.tick().await;
        let cooling_cmd = freq_rx.try_recv().unwrap();
        assert_eq!(cooling_cmd, FrequencyCommand::BumpDown);

        temp_tx
            .send(Some(config.target_temperature_c + 1.0))
            .unwrap();
        controller.tick().await;
        let throttling_cmd = freq_rx.try_recv().unwrap();
        assert_eq!(throttling_cmd, FrequencyCommand::BumpDown);

        temp_tx.send(Some(config.max_temperature_c + 1.0)).unwrap();
        controller.tick().await;
        let critical_cmd = freq_rx.try_recv().unwrap();
        assert_eq!(critical_cmd, FrequencyCommand::BumpDown);
    }

    #[tokio::test]
    async fn should_send_bump_up_when_state_becomes_less_severe() {
        let (mut controller, _fan_rx, mut freq_rx, temp_tx) = create_controller();
        let config = ThermalConfig::default();

        controller.set_state(ThermalState::CRITICAL);
        temp_tx
            .send(Some(config.max_temperature_c - HYSTERESIS_C - 1.0))
            .unwrap();
        controller.tick().await;
        let throttling_cmd = freq_rx.try_recv().unwrap();
        assert_eq!(throttling_cmd, FrequencyCommand::BumpUp);

        temp_tx
            .send(Some(config.target_temperature_c - HYSTERESIS_C - 1.0))
            .unwrap();
        controller.tick().await;
        let cooling_cmd = freq_rx.try_recv().unwrap();
        assert_eq!(cooling_cmd, FrequencyCommand::BumpUp);

        temp_tx
            .send(Some(NORMAL_THRESHOLD_C - HYSTERESIS_C - 1.0))
            .unwrap();
        controller.tick().await;
        let normal_cmd = freq_rx.try_recv().unwrap();
        assert_eq!(normal_cmd, FrequencyCommand::BumpUp);
    }

    #[tokio::test]
    async fn should_freeze_integral_in_normal_and_critical_states() {
        let (mut controller, _fan_rx, _freq_rx, temp_tx) = create_controller();
        let config = ThermalConfig::default();

        controller.set_fan_pid_integral_sum(5.0);
        let initial_integral = controller.fan_pid_integral_sum();

        temp_tx.send(Some(NORMAL_THRESHOLD_C - 1.0)).unwrap();
        controller.tick().await;

        assert!((controller.fan_pid_integral_sum() - initial_integral).abs() < 1e-3);

        temp_tx.send(Some(config.max_temperature_c + 1.0)).unwrap();
        controller.tick().await;

        assert!((controller.fan_pid_integral_sum() - initial_integral).abs() < 1e-3);
    }

    #[tokio::test]
    async fn should_accumulate_integral_in_cooling_and_throttling_states() {
        let (mut controller, _fan_rx, _freq_rx, temp_tx) = create_controller();
        let config = ThermalConfig::default();

        controller.set_fan_pid_integral_sum(0.0);
        controller.set_tick_duration(Duration::from_millis(100));

        temp_tx.send(Some(NORMAL_THRESHOLD_C + 1.0)).unwrap();
        controller.tick().await;

        let integral_after_cooling = controller.fan_pid_integral_sum();
        assert!(integral_after_cooling < 0.0);

        temp_tx
            .send(Some(config.target_temperature_c + 1.0))
            .unwrap();
        controller.tick().await;

        assert!(controller.fan_pid_integral_sum() > integral_after_cooling);
    }
}
