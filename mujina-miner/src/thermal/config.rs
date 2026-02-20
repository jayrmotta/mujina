#[derive(Debug, Clone)]
pub struct ThermalConfig {
    /// Fan PID target (°C). Adjust based on cooling capacity and
    /// ambient conditions.
    pub target_temperature_c: f32,

    /// Frequency throttling threshold (°C). Must be higher than
    /// `target_temperature_c`.
    pub max_temperature_c: f32,

    /// Target chip clock after initialization (MHz). Higher values
    /// increase hashrate and power draw.
    pub operating_frequency_mhz: f32,
}

impl Default for ThermalConfig {
    fn default() -> Self {
        Self {
            target_temperature_c: 74.0,
            max_temperature_c: 85.0,
            operating_frequency_mhz: 525.0,
        }
    }
}
