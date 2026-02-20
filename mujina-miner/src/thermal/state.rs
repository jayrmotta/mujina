use super::config::ThermalConfig;

pub(super) const NORMAL_THRESHOLD_C: f32 = 55.0;

/// Prevents oscillation between thermal states on noisy readings.
pub(super) const HYSTERESIS_C: f32 = 2.0;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ThermalState {
    NORMAL,
    COOLING,
    THROTTLING,
    CRITICAL,
}

impl ThermalState {
    pub fn from_temperature(temp: f32, previous: ThermalState, config: &ThermalConfig) -> Self {
        match previous {
            ThermalState::NORMAL => {
                if temp > NORMAL_THRESHOLD_C {
                    ThermalState::COOLING
                } else {
                    ThermalState::NORMAL
                }
            }
            ThermalState::COOLING => {
                if temp <= NORMAL_THRESHOLD_C - HYSTERESIS_C {
                    ThermalState::NORMAL
                } else if temp > config.target_temperature_c {
                    ThermalState::THROTTLING
                } else {
                    ThermalState::COOLING
                }
            }
            ThermalState::THROTTLING => {
                if temp <= config.target_temperature_c - HYSTERESIS_C {
                    ThermalState::COOLING
                } else if temp > config.max_temperature_c {
                    ThermalState::CRITICAL
                } else {
                    ThermalState::THROTTLING
                }
            }
            ThermalState::CRITICAL => {
                if temp <= config.max_temperature_c - HYSTERESIS_C {
                    ThermalState::THROTTLING
                } else {
                    ThermalState::CRITICAL
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thermal::ThermalConfig;

    #[test]
    fn should_be_normal_thermal_state() {
        let config = ThermalConfig::default();
        let state =
            ThermalState::from_temperature(NORMAL_THRESHOLD_C, ThermalState::NORMAL, &config);
        assert_eq!(state, ThermalState::NORMAL);
    }

    #[test]
    fn should_be_cooling_thermal_state() {
        let config = ThermalConfig::default();
        let state = ThermalState::from_temperature(
            config.target_temperature_c,
            ThermalState::COOLING,
            &config,
        );
        assert_eq!(state, ThermalState::COOLING);
    }

    #[test]
    fn should_be_throttling_thermal_state() {
        let config = ThermalConfig::default();
        let state = ThermalState::from_temperature(
            config.max_temperature_c,
            ThermalState::THROTTLING,
            &config,
        );
        assert_eq!(state, ThermalState::THROTTLING);
    }

    #[test]
    fn should_be_critical_thermal_state() {
        let config = ThermalConfig::default();
        let state = ThermalState::from_temperature(
            config.max_temperature_c + 1.0,
            ThermalState::CRITICAL,
            &config,
        );
        assert_eq!(state, ThermalState::CRITICAL);
    }
}
