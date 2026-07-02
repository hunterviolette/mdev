#[derive(Debug, Clone)]
pub struct StageCapabilities {
    keys: Vec<&'static str>,
}

impl StageCapabilities {
    pub fn new<const N: usize>(keys: [&'static str; N]) -> Self {
        Self {
            keys: keys.into_iter().collect(),
        }
    }

    pub fn empty() -> Self {
        Self { keys: Vec::new() }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.keys.iter().any(|item| *item == key)
    }

    pub fn keys(&self) -> &[&'static str] {
        &self.keys
    }
}
