//! The framework's HTTP middleware.

mod cors;
mod hosts;
mod maintenance;
mod precognition;
mod transform;

pub use cors::{CorsOptions, HandleCors};
pub use hosts::{AddLinkHeadersForPreloadedAssets, TrustHosts, ip_matches};
pub use maintenance::{MaintenanceModeBypassCookie, PreventRequestsDuringMaintenance, ServePublicFiles, public_file};
pub use precognition::HandlePrecognitiveRequests;
pub use transform::{ConvertEmptyStringsToNull, FrameGuard, SetCacheHeaders, TrimStrings, TrustProxies};
