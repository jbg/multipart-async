// Copyright 2017-2019 `multipart-async` Crate Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::{
    fmt,
    future::Future,
    mem,
    pin::Pin,
    rc::Rc,
    str,
    task::{Context, Poll::{self, *}},
};

use futures_core::{Stream, TryStream};

use crate::server::{Error, Error::Utf8, PushChunk};
use crate::BodyChunk;

use super::boundary::BoundaryFinder;
use super::Multipart;

pub use self::headers::FieldHeaders;
pub(crate) use self::headers::ReadHeaders;

// mod collect;
mod headers;

/// A `Future` potentially yielding the next field in the multipart stream.
///
/// If there are no more fields in the stream, `Ok(None)` is returned.
///
/// See [`Multipart::next_field()`](../struct.Multipart.html#method.next_field) for usage.
pub struct NextField<'a, S: TryStream + 'a> {
    multipart: Option<Pin<&'a mut Multipart<S>>>,
    has_next_field: bool,
}

pub struct IntoNextField<S: TryStream> {
    multipart: Option<Multipart<S>>,
    has_next_field: bool,
}

impl<'a, S: TryStream + 'a> NextField<'a, S> {
    pub(crate) fn new(multipart: Pin<&'a mut Multipart<S>>) -> Self {
        NextField {
            multipart: Some(multipart),
            has_next_field: false,
        }
    }
}

impl<S: TryStream> IntoNextField<S> {
    pub(crate) fn new(multipart: Multipart<S>) -> Self {
        IntoNextField {
            multipart: Some(multipart),
            has_next_field: false,
        }
    }
}

impl<'a, S: 'a> Future for NextField<'a, S>
where
    S: TryStream,
    S::Ok: BodyChunk,
    Error<S::Error>: From<S::Error>,
{
    type Output = super::Result<Option<Field<FieldDataBorrowed<'a, S>>>, S::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // since we can't use `?` with `Option<...>` in this context
        macro_rules! multipart {
            (get) => {
                if let Some(ref mut multipart) = self.multipart {
                    multipart.as_mut()
                } else {
                    return Ready(Ok(None));
                }
            };
            (take) => {
                if let Some(multipart) = self.multipart.take() {
                    multipart
                } else {
                    return Ready(Ok(None));
                }
            };
        }

        // `false` and `multipart = Some` means we haven't polled for next field yet
        self.has_next_field =
            self.has_next_field || ready!(multipart!(get).poll_has_next_field(cx)?);

        if !self.has_next_field {
            // end of stream, next `?` will return
            self.multipart = None;
        }

        Ready(Ok(Some(Field {
            headers: ready!(multipart!(get).poll_field_headers(cx)?),
            data: FieldDataBorrowed {
                multipart: multipart!(take),
            },
            _priv: (),
        })))
    }
}

impl<S> Future for IntoNextField<S>
where
    S: TryStream + Unpin + 'static,
    S::Ok: BodyChunk + Unpin,
    Error<S::Error>: From<S::Error>,
{
    type Output = super::Result<Option<Field<FieldDataOwned<S>>>, S::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // since we can't use `?` with `Option<...>` in this context
        macro_rules! multipart {
            (get) => {
                if let Some(ref mut multipart) = self.multipart {
                    Pin::new(multipart)
                } else {
                    return Ready(Ok(None));
                }
            };
            (take) => {
                if let Some(multipart) = self.multipart.take() {
                    multipart
                } else {
                    return Ready(Ok(None));
                }
            };
        }

        // `false` and `multipart = Some` means we haven't polled for next field yet
        self.has_next_field =
            self.has_next_field || ready!(multipart!(get).poll_has_next_field(cx)?);

        if !self.has_next_field {
            // end of stream, next `?` will return
            self.multipart = None;
        }

        Ready(Ok(Some(Field {
            headers: ready!(multipart!(get).poll_field_headers(cx)?),
            data: FieldDataOwned {
                multipart: multipart!(take),
            },
            _priv: (),
        })))
    }
}

/// A single field in a multipart stream.
///
/// The data of the field is provided as a `Stream` impl in the `data` field.
pub struct Field<D> {
    /// The headers of this field, including the name, filename, and `Content-Type`, if provided.
    pub headers: FieldHeaders,
    /// The data of this field in the request, represented as a stream of chunks.
    pub data: D,
    _priv: (),
}

impl<D> fmt::Debug for Field<D> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Field")
            .field("headers", &self.headers)
            .field("data", &"<FieldData>")
            .finish()
    }
}

/// The data of a field in a multipart stream, as a stream of chunks.
///
/// It may be read to completion via the `Stream` impl, or collected to a string with
/// `.read_to_string()`.
pub struct FieldDataBorrowed<'a, S: TryStream + 'a> {
    multipart: Pin<&'a mut Multipart<S>>,
}

pub struct FieldDataOwned<S: TryStream> {
    multipart: Multipart<S>,
}

pub trait FieldData<'a, S: TryStream> {
    fn read_to_string(self) -> ReadToString<Self>
    where
        Self: TryStream + Unpin + Sized
    {
        ReadToString::new(self)
    }
}

impl<'a, S: TryStream + 'a> FieldData<'a, S> for FieldDataBorrowed<'a, S> {}

impl<S: TryStream> FieldData<'static, S> for FieldDataOwned<S> {}

impl<S: TryStream> Stream for FieldDataBorrowed<'_, S>
where
    S::Ok: BodyChunk,
    Error<S::Error>: From<S::Error>,
{
    type Item = super::Result<S::Ok, S::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        self.multipart.as_mut().poll_field_chunk(cx)
    }
}

impl<S: TryStream> Stream for FieldDataOwned<S>
where
    S: Unpin,
    S::Ok: BodyChunk + Unpin,
    Error<S::Error>: From<S::Error>,
{
    type Item = super::Result<S::Ok, S::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.multipart).poll_field_chunk(cx)
    }
}

/// A `Future` that yields the body of a field read to a `String`.
pub struct ReadToString<S: TryStream + Unpin> {
    stream: S,
    string: String,
    surrogate: Option<([u8; 3], u8)>,
}

impl<S: TryStream + Unpin> ReadToString<S> {
    pub(crate) fn new(stream: S) -> Self {
        ReadToString {
            stream,
            string: String::new(),
            surrogate: None,
        }
    }
}

impl<S: TryStream + Unpin> Future for ReadToString<S>
where
    S::Ok: BodyChunk,
    Error<S::Error>: From<S::Error>,
{
    type Output = super::Result<String, S::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        while let Some(mut data) = ready!(Pin::new(&mut self.stream).try_poll_next(cx)?) {
            if let Some((mut start, start_len)) = self.surrogate {
                assert!(
                    start_len > 0 && start_len < 4,
                    "start_len out of range: {:?}",
                    start_len
                );

                let start_len = start_len as usize;

                let (width, needed) = if let Some(width) = utf8_char_width(start[0]) {
                    (
                        width,
                        width.checked_sub(start_len).expect("start_len >= width"),
                    )
                } else {
                    return Ready(fmt_err!(
                        "unexpected start of UTF-8 surrogate: {:X}",
                        start[0]
                    ));
                };

                if data.len() < needed {
                    start[start_len..start_len + data.len()].copy_from_slice(data.slice(..));
                    self.surrogate = Some((start, (start_len + data.len()) as u8));
                    continue;
                }

                let mut surrogate = [0u8; 4];
                surrogate[..start_len].copy_from_slice(&start[..start_len]);
                surrogate[start_len..width].copy_from_slice(data.slice(..needed));

                trace!("decoding surrogate: {:?}", &surrogate[..width]);

                self.string
                    .push_str(str::from_utf8(&surrogate[..width]).map_err(Utf8)?);

                let (_, rem) = data.split_into(needed);
                data = rem;
                self.surrogate = None;
            }

            match str::from_utf8(data.as_slice()) {
                Ok(s) => self.string.push_str(s),
                Err(e) => {
                    if e.error_len().is_some() {
                        trace!("ReadToString failed to decode; string: {:?}, surrogate: {:?}, data: {:?}",
                           self.string, self.surrogate, data.as_slice());
                        // we encountered an invalid surrogate
                        return Ready(Err(Utf8(e)));
                    } else {
                        self.string.push_str(unsafe {
                            // Utf8Error specifies that `..e.valid_up_to()` is valid UTF-8
                            str::from_utf8_unchecked(data.slice(..e.valid_up_to()))
                        });

                        let start_len = data.len() - e.valid_up_to();
                        let mut start = [0u8; 3];
                        start[..start_len].copy_from_slice(data.slice(e.valid_up_to()..));

                        // `e.valid_up_to()` is specified to be `[-1, -3]` of `data.len()`
                        self.surrogate = Some((start, start_len as u8));
                    }
                }
            }
        }

        if let Some((start, _)) = self.surrogate {
            ret_err!("incomplete UTF-8 surrogate: {:?}", start);
        }

        Ready(Ok(mem::replace(&mut self.string, String::new())))
    }
}

fn utf8_char_width(first: u8) -> Option<usize> {
    // simplification of the LUT here:
    // https://github.com/rust-lang/rust/blob/fe6d05a/src/libcore/str/mod.rs#L1565
    match first {
        // ASCII characters are one byte
        0x00..=0x7F => Some(1),
        0xC2..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF4 => Some(4),
        _ => None,
    }
}

#[test]
fn test_read_to_string() {
    use crate::test_util::mock_stream;
    use futures_util::TryFutureExt;

    let _ = ::env_logger::try_init();

    let test_data = mock_stream(&[b"Hello", b",", b" ", b"world!"]);

    let mut read_to_string = ReadToString::new(test_data);

    ready_assert_eq!(
        |cx| read_to_string.try_poll_unpin(cx),
        Ok("Hello, world!".to_string())
    );

    let test_data_unicode = mock_stream(&[
        &[40, 226, 149],
        &[175, 194, 176, 226, 150],
        &[161, 194, 176, 41, 226, 149],
        &[175, 239, 184, 181, 32],
        &[226, 148, 187, 226, 148, 129, 226, 148, 187],
    ]);

    let mut read_to_string = ReadToString::new(test_data_unicode);

    ready_assert_eq!(
        |cx| read_to_string.try_poll_unpin(cx),
        Ok("(╯°□°)╯︵ ┻━┻".to_string())
    );
}
