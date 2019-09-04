// Copyright 2017-2 `multipart-async` Crate Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.
//! Client- and server-side abstractions for HTTP `multipart/form-data` requests using asynchronous
//! I/O.
//!
//! Features:
//!
//! * `client` (default): Enable the client-side abstractions for multipart requests. If the
//! `hyper` feature is also set, enables integration with the Hyper HTTP client API.
//!
//! * `server` (default): Enable the server-side abstractions for multipart requests. If the
//! `hyper` feature is also set, enables integration with the Hyper HTTP server API.
#![allow(unused_imports, deprecated)]
// FIXME: hiding irrelevant warnings during prototyping
// #![deny(missing_docs)]

#[macro_use]
extern crate log;
//extern crate env_logger;

#[macro_use]
extern crate futures;

#[macro_use]
extern crate pin_utils;

pub extern crate mime;

pub extern crate http;

#[cfg(test)]
#[macro_use]
extern crate lazy_static;

use rand::Rng;

use futures::{Future, Stream};
use std::borrow::Cow;
use std::process::Output;
use std::str::Utf8Error;
use std::{io, ops};

#[cfg(any(test, fuzzing))]
#[macro_use]
mod test_util;

// FIXME: after server prototype is working
//#[cfg(feature = "client")]
//pub mod client;

#[cfg(feature = "server")]
pub mod server;

#[cfg(any(test, feature = "fuzzing"))]
pub mod fuzzing;

mod helpers;

/*#[cfg(all(test, feature = "client", feature = "server"))]
mod local_test;
*/
fn random_alphanumeric(len: usize) -> String {
    rand::thread_rng().gen_ascii_chars().take(len).collect()
}

/// The operations required from a body stream's `Item` type.
pub trait BodyChunk: Sized {
    /// Split the chunk at `idx`, returning `(self[..idx], self[idx..])`.
    fn split_at(self, idx: usize) -> (Self, Self);

    /// Get the slice representing the data of this chunk.
    fn as_slice(&self) -> &[u8];

    /// Equivalent to `self.as_slice().len()`
    #[inline(always)]
    fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Equivalent to `self.as_slice().is_empty()`
    #[inline(always)]
    fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    /// Equivalent to `self.as_slice().to_owned()`
    ///
    /// Implementors are welcome to override this if they can provide a cheaper conversion.
    #[inline(always)]
    fn into_vec(self) -> Vec<u8> {
        self.as_slice().to_owned()
    }
}

impl BodyChunk for Vec<u8> {
    fn split_at(mut self, idx: usize) -> (Self, Self) {
        let other = self.split_off(idx);
        (self, other)
    }

    fn as_slice(&self) -> &[u8] {
        self
    }

    fn into_vec(self) -> Vec<u8> {
        self
    }
}

impl<'a> BodyChunk for &'a [u8] {
    fn split_at(self, idx: usize) -> (Self, Self) {
        self.split_at(idx)
    }

    fn as_slice(&self) -> &[u8] {
        self
    }
}

impl<'a> BodyChunk for Cow<'a, [u8]> {
    fn split_at(self, idx: usize) -> (Self, Self) {
        fn cow_tup<'a, T: Into<Cow<'a, [u8]>>>(
            (left, right): (T, T),
        ) -> (Cow<'a, [u8]>, Cow<'a, [u8]>) {
            (left.into(), right.into())
        }

        match self {
            Cow::Borrowed(slice) => cow_tup(slice.split_at(idx)),
            Cow::Owned(vec) => cow_tup(vec.split_at(idx)),
        }
    }

    fn as_slice(&self) -> &[u8] {
        &**self
    }
}

impl BodyChunk for ::bytes::Bytes {
    #[inline]
    fn split_at(mut self, idx: usize) -> (Self, Self) {
        (self.split_to(idx), self)
    }

    #[inline]
    fn as_slice(&self) -> &[u8] {
        self
    }
}

/// The operations required from a body stream's `Error` type.
pub trait StreamError: From<io::Error> {
    /// Wrap a static string into this error type.
    ///
    /// Goes through `io::Error` by default.
    fn from_str(str: &'static str) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, str).into()
    }

    /// Wrap a dynamic string into this error type.
    ///
    /// Goes through `io::Error` by default.
    fn from_string(string: String) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, string).into()
    }

    /// Wrap a `std::str::Utf8Error` into this error type.
    ///
    /// Goes through `io::Error` by default.
    fn from_utf8(err: Utf8Error) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, err).into()
    }
}

impl StreamError for io::Error {}

/// Not a stable API
#[doc(hidden)]
#[derive(Debug, Eq, PartialEq)]
pub struct StringError(String);

impl crate::StreamError for StringError {
    fn from_str(str: &'static str) -> Self {
        StringError(str.into())
    }

    fn from_string(string: String) -> Self {
        StringError(string)
    }
}

impl Into<String> for StringError {
    fn into(self) -> String {
        self.0
    }
}

impl From<std::io::Error> for StringError {
    fn from(err: std::io::Error) -> Self {
        StringError(err.to_string())
    }
}

impl PartialEq<str> for StringError {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl<'a> PartialEq<&'a str> for StringError {
    fn eq(&self, other: &&'a str) -> bool {
        self.0 == *other
    }
}
