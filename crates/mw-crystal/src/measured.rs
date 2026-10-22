/// A measured value with its optional standard uncertainty, as CIF writes
/// `3.905(2)`. A value typed by the user has no uncertainty.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measured {
    value: f64,
    su: Option<f64>,
}

impl Measured {
    pub fn exact(value: f64) -> Self {
        Self { value, su: None }
    }

    pub fn with_su(value: f64, su: f64) -> Self {
        Self {
            value,
            su: Some(su),
        }
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn su(&self) -> Option<f64> {
        self.su
    }
}

impl From<f64> for Measured {
    fn from(value: f64) -> Self {
        Self::exact(value)
    }
}
