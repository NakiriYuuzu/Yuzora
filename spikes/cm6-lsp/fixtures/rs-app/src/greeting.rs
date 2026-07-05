pub struct Greeting {
    pub text: String,
    pub times: u32,
}

pub fn build_greeting(name: &str, times: u32) -> Greeting {
    let text = format!("Hello, {}!", name);
    Greeting { text, times }
}

pub fn repeat_greeting(greeting: &Greeting) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for _ in 0..greeting.times {
        lines.push(greeting.text.clone());
    }
    lines
}
