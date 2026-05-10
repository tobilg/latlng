#![forbid(unsafe_code)]

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

pub mod latlng_capnp {
    include!(concat!(env!("OUT_DIR"), "/latlng_capnp.rs"));
}
