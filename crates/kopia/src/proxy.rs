//! Loopback SigV4 re-signing proxy for kopia's S3 backend.
//!
//! kopia talks to this proxy over plain HTTP with meaningless dummy
//! credentials; the proxy discards the dummy signature, re-signs each request
//! with live credentials drawn from a [`CredentialProvider`], and forwards it to
//! real S3 over TLS. A long-running kopia operation (maintenance, a large backup
//! or restore) then outlives any single set of short-lived credentials: each
//! request is signed afresh with whatever the provider currently holds, and a
//! refresh between two requests is invisible to kopia.
//!
//! See the S3P spec for the design.

pub mod sigv4;
pub mod stream;
