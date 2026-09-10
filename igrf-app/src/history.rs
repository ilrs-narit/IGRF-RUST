/// Maximum points kept in one history series before the oldest are dropped.
const HISTORY_LIMIT: usize = 500;

#[derive(Default)]
pub struct History {
    points: Vec<[f64; 2]>,
}

impl History {
    pub fn push(&mut self, x: f64, y: f64) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        self.points.push([x, y]);
        if self.points.len() > HISTORY_LIMIT {
            let extra = self.points.len() - HISTORY_LIMIT;
            self.points.drain(..extra);
        }
    }

    pub fn clear(&mut self) {
        self.points.clear();
    }

    pub fn points(&self) -> &[[f64; 2]] {
        &self.points
    }
}

#[derive(Default)]
pub struct PlotHistory {
    pub sensor_setpoint: [History; 3],
    pub sensor_measured: [History; 3],
    pub sensor_magnitude_setpoint: History,
    pub sensor_magnitude_measured: History,
    pub magson: [History; 4],
}

impl PlotHistory {
    pub fn clear(&mut self) {
        for history in &mut self.sensor_setpoint {
            history.clear();
        }
        for history in &mut self.sensor_measured {
            history.clear();
        }
        self.sensor_magnitude_setpoint.clear();
        self.sensor_magnitude_measured.clear();
        for history in &mut self.magson {
            history.clear();
        }
    }
}
