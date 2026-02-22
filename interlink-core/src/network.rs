pub struct NetworkClient {
    pub url: String,
}

impl NetworkClient {
    pub fn new(url: String) -> Self {
        Self { url }
    }

    pub fn connect(&self) {
        println!("Connecting to {}", self.url);
    }
}
