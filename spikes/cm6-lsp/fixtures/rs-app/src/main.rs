mod greeting;

use greeting::{build_greeting, repeat_greeting, Greeting};

fn main() {
    let greeting: Greeting = build_greeting("Yuzora", 3);
    let lines = repeat_greeting(&greeting);
    for line in &lines {
        println!("{}", line);
    }
}
