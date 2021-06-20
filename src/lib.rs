mod config;
mod generator;
mod manifest;
mod theme;

pub use config::Config;
pub use generator::Generator;
pub use manifest::Manifest;
pub use theme::Theme;

pub fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
