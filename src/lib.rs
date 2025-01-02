pub mod cli;
pub mod telemetry;

/// Global memory allocator backed by [jemalloc].
///
/// This static variable is exposed solely for the documentation purposes and don't need to be used
/// directly. If **jemalloc** feature is enabled then the service will use jemalloc for all the
/// memory allocations implicitly.
///
/// If no Foundations API is being used by your project, you will need to explicitly link foundations crate
/// to your project by adding `extern crate foundations;` to your `main.rs` or `lib.rs`, for jemalloc to
/// be embedded in your binary.
///
/// [jemalloc]: https://github.com/jemalloc/jemalloc
#[cfg(feature = "jemalloc")]
#[global_allocator]
pub static JEMALLOC_MEMORY_ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// Basic service information.
#[derive(Clone, Debug, Default)]
pub struct ServiceInfo {
    /// The name of the service.
    pub name: &'static str,

    /// The service identifier as used in metrics.
    ///
    /// Usually the same as [`ServiceInfo::name`], with hyphens (`-`) replaced by underscores `_`.
    pub name_in_metrics: String,

    /// The version of the service.
    pub version: &'static str,

    /// Service author.
    pub author: &'static str,
    /// The description of the service.
    pub description: &'static str,
}

/// Creates [`ServiceInfo`] from the information in `Cargo.toml` manifest of the service.
///
/// [`ServiceInfo::name_in_metrics`] is the same as the package name, with hyphens (`-`) replaced
/// by underscores (`_`).
#[macro_export]
macro_rules! service_info {
    () => {
        $crate::ServiceInfo {
            name: env!("CARGO_PKG_NAME"),
            name_in_metrics: env!("CARGO_PKG_NAME").replace("-", "_"),
            version: env!("CARGO_PKG_VERSION"),
            author: env!("CARGO_PKG_AUTHORS"),
            description: env!("CARGO_PKG_DESCRIPTION"),
        }
    };
}
