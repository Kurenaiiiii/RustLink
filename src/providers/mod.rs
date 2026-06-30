mod google_tts;
mod http;
mod local;

pub use google_tts::GoogleTtsProvider;
pub use http::HttpProvider;
pub use local::LocalProvider;
