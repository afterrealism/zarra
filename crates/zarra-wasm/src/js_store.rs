#[cfg(target_arch = "wasm32")]
use futures::{stream, StreamExt};
#[cfg(target_arch = "wasm32")]
use js_sys::{Function, Promise, Reflect, Uint8Array};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::JsFuture;
#[cfg(target_arch = "wasm32")]
use zarrs::storage::byte_range::{ByteRange, ByteRangeIterator};
#[cfg(target_arch = "wasm32")]
use zarrs::storage::{
    AsyncMaybeBytesIterator, AsyncReadableStorageTraits, Bytes, MaybeBytes, StorageError, StoreKey,
};

#[cfg(target_arch = "wasm32")]
pub(crate) struct JsStore {
    get: Function,
}

#[cfg(target_arch = "wasm32")]
impl JsStore {
    pub(crate) fn new(get: Function) -> Self {
        Self { get }
    }

    async fn request(
        &self,
        key: &StoreKey,
        range: Option<String>,
    ) -> Result<ObjectResponse, StorageError> {
        let range = range.map_or(JsValue::NULL, |value| JsValue::from_str(&value));
        let result = self
            .get
            .call2(
                &JsValue::UNDEFINED,
                &JsValue::from_str(key.as_str()),
                &range,
            )
            .map_err(|_| StorageError::Other("object store call failed".into()))?;
        let result = JsFuture::from(Promise::resolve(&result))
            .await
            .map_err(|_| StorageError::Other("object store request failed".into()))?;
        ObjectResponse::decode(result)
    }
}

#[cfg(target_arch = "wasm32")]
struct ObjectResponse {
    status: u16,
    bytes: Uint8Array,
    content_range: Option<String>,
}

#[cfg(target_arch = "wasm32")]
impl ObjectResponse {
    fn decode(value: JsValue) -> Result<Self, StorageError> {
        let status = number(&value, "status")?;
        let status = u16::try_from(status)
            .map_err(|_| StorageError::Other("object store returned an invalid status".into()))?;
        let content_length = number(&value, "contentLength")?;
        let bytes = Uint8Array::new(
            &Reflect::get(&value, &JsValue::from_str("bytes"))
                .map_err(|_| StorageError::Other("object store response has no bytes".into()))?,
        );
        if u64::from(bytes.length()) != content_length {
            return Err(StorageError::Other(
                "object store response length is inconsistent".into(),
            ));
        }
        let content_range = match Reflect::get(&value, &JsValue::from_str("contentRange")) {
            Ok(value) if value.is_null() || value.is_undefined() => None,
            Ok(value) => value.as_string(),
            Err(_) => None,
        };
        Ok(Self {
            status,
            bytes,
            content_range,
        })
    }
}

#[cfg(target_arch = "wasm32")]
fn number(value: &JsValue, field: &str) -> Result<u64, StorageError> {
    Reflect::get(value, &JsValue::from_str(field))
        .ok()
        .and_then(|value| value.as_f64())
        .filter(|value| {
            value.is_finite()
                && *value >= 0.0
                && *value <= 9_007_199_254_740_991.0
                && value.fract() == 0.0
        })
        .map(|value| value as u64)
        .ok_or_else(|| StorageError::Other(format!("object store response has invalid {field}")))
}

#[cfg(target_arch = "wasm32")]
fn range_header(range: ByteRange) -> Result<String, StorageError> {
    match range {
        ByteRange::FromStart(offset, None) => Ok(format!("bytes={offset}-")),
        ByteRange::FromStart(_, Some(0)) | ByteRange::Suffix(0) => Err(StorageError::Other(
            "empty HTTP byte ranges are not supported".into(),
        )),
        ByteRange::FromStart(offset, Some(length)) => {
            let end = offset
                .checked_add(length - 1)
                .ok_or_else(|| StorageError::Other("HTTP byte range overflows u64".into()))?;
            Ok(format!("bytes={offset}-{end}"))
        }
        ByteRange::Suffix(length) => Ok(format!("bytes=-{length}")),
    }
}

#[cfg(target_arch = "wasm32")]
fn validate_partial(range: ByteRange, response: &ObjectResponse) -> Result<(), StorageError> {
    let content_range = response
        .content_range
        .as_deref()
        .ok_or_else(|| StorageError::Other("206 response has no Content-Range".into()))?;
    let (actual, total) = parse_content_range_total(content_range)
        .map_err(|error| StorageError::Other(error.to_string()))?;
    let (expected, expected_length) = match range {
        ByteRange::FromStart(offset, None) => {
            let end = total
                .checked_sub(1)
                .filter(|_| offset < total)
                .ok_or_else(|| StorageError::Other("open range exceeds the object size".into()))?;
            (format!("{offset}-{end}"), total - offset)
        }
        ByteRange::FromStart(offset, Some(length)) => {
            let end = offset
                .checked_add(length)
                .and_then(|exclusive| exclusive.checked_sub(1))
                .filter(|_| length > 0)
                .filter(|end| *end < total)
                .ok_or_else(|| {
                    StorageError::Other("bounded range exceeds the object size".into())
                })?;
            (format!("{offset}-{end}"), length)
        }
        ByteRange::Suffix(length) => {
            let end = total.checked_sub(1).filter(|_| length > 0).ok_or_else(|| {
                StorageError::Other("suffix range exceeds the object size".into())
            })?;
            let start = total.saturating_sub(length);
            (format!("{start}-{end}"), total - start)
        }
    };
    if actual != expected {
        return Err(StorageError::Other(
            "partial response Content-Range does not match the request".into(),
        ));
    }
    if u64::from(response.bytes.length()) != expected_length {
        return Err(StorageError::Other(
            "partial response body length does not match Content-Range".into(),
        ));
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
#[async_trait::async_trait(?Send)]
impl AsyncReadableStorageTraits for JsStore {
    async fn get(&self, key: &StoreKey) -> Result<MaybeBytes, StorageError> {
        let response = self.request(key, None).await?;
        match response.status {
            200 => Ok(Some(Bytes::from(response.bytes.to_vec()))),
            404 => Ok(None),
            status => Err(StorageError::Other(format!(
                "full object GET requires 200, received {status}"
            ))),
        }
    }

    async fn get_partial(
        &self,
        key: &StoreKey,
        byte_range: ByteRange,
    ) -> Result<MaybeBytes, StorageError> {
        let response = self.request(key, Some(range_header(byte_range)?)).await?;
        match response.status {
            206 => {
                validate_partial(byte_range, &response)?;
                Ok(Some(Bytes::from(response.bytes.to_vec())))
            }
            404 => Ok(None),
            status => Err(StorageError::Other(format!(
                "partial object GET requires 206, received {status}"
            ))),
        }
    }

    async fn get_partial_many<'a>(
        &'a self,
        key: &StoreKey,
        byte_ranges: ByteRangeIterator<'a>,
    ) -> Result<AsyncMaybeBytesIterator<'a>, StorageError> {
        let mut values = Vec::new();
        for range in byte_ranges {
            let Some(bytes) = self.get_partial(key, range).await? else {
                return Ok(None);
            };
            values.push(Ok(bytes));
        }
        Ok(Some(stream::iter(values).boxed()))
    }

    async fn size_key(&self, key: &StoreKey) -> Result<Option<u64>, StorageError> {
        let response = self.request(key, Some("bytes=0-0".into())).await?;
        match response.status {
            206 => {
                validate_partial(ByteRange::FromStart(0, Some(1)), &response)?;
                response
                    .content_range
                    .as_deref()
                    .ok_or_else(|| StorageError::Other("206 response has no Content-Range".into()))
                    .and_then(|value| {
                        parse_content_range_total(value)
                            .map_err(|error| StorageError::Other(error.to_string()))
                    })
                    .map(|(_, total)| Some(total))
            }
            404 => Ok(None),
            416 if response.content_range.as_deref() == Some("bytes */0") => Ok(Some(0)),
            status => Err(StorageError::Other(format!(
                "size probe requires 206, 404, or empty 416, received {status}"
            ))),
        }
    }

    fn supports_get_partial(&self) -> bool {
        true
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
fn parse_content_range_total(value: &str) -> Result<(&str, u64), &'static str> {
    let value = value
        .strip_prefix("bytes ")
        .ok_or("Content-Range must use bytes")?;
    let (range, total) = value.rsplit_once('/').ok_or("Content-Range has no total")?;
    let total = total
        .parse::<u64>()
        .map_err(|_| "Content-Range total is invalid")?;
    if range == "*" {
        if total == 0 {
            return Ok((range, total));
        }
        return Err("unsatisfied Content-Range must have total zero");
    }
    let (start, end) = range.split_once('-').ok_or("Content-Range has no bounds")?;
    let start = start
        .parse::<u64>()
        .map_err(|_| "Content-Range start is invalid")?;
    let end = end
        .parse::<u64>()
        .map_err(|_| "Content-Range end is invalid")?;
    if start > end || end >= total {
        return Err("Content-Range bounds are invalid");
    }
    Ok((range, total))
}

#[cfg(test)]
mod tests {
    use super::parse_content_range_total;

    #[test]
    fn content_range_parser_accepts_only_bounded_ranges() {
        assert_eq!(parse_content_range_total("bytes 0-0/123"), Ok(("0-0", 123)));
        assert_eq!(parse_content_range_total("bytes */0"), Ok(("*", 0)));
        for invalid in [
            "bytes 0-0/0",
            "bytes 2-1/123",
            "bytes 0-123/123",
            "bytes */123",
            "items 0-0/123",
            "bytes 0-0/*",
        ] {
            assert!(parse_content_range_total(invalid).is_err(), "{invalid}");
        }
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use js_sys::Function;
    use wasm_bindgen::JsValue;
    use wasm_bindgen_test::wasm_bindgen_test;
    use zarrs::storage::byte_range::ByteRange;
    use zarrs::storage::{AsyncReadableStorageTraits, StoreKey};

    use super::JsStore;

    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    async fn size_key_uses_a_single_byte_get_and_accepts_empty_416() -> Result<(), JsValue> {
        let get = Function::new_with_args(
            "key, range",
            "if (range !== 'bytes=0-0') throw new Error('wrong range');\n\
             const empty = key === 'empty';\n\
             return Promise.resolve({ status: empty ? 416 : 206, bytes: new Uint8Array(empty ? 0 : 1), contentRange: empty ? 'bytes */0' : 'bytes 0-0/123', contentLength: empty ? 0 : 1 });",
        );
        let store = JsStore::new(get);
        for (name, expected) in [("empty", Some(0)), ("present", Some(123))] {
            let key = StoreKey::new(name).map_err(|_| JsValue::from_str("invalid test key"))?;
            let size = store
                .size_key(&key)
                .await
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            assert_eq!(size, expected);
        }
        Ok(())
    }

    #[wasm_bindgen_test]
    async fn partial_get_checks_exact_range_and_body_length() -> Result<(), JsValue> {
        let get = Function::new_with_args(
            "key, range",
            "if (range !== 'bytes=2-3') throw new Error('wrong range');\n\
             const valid = key === 'valid';\n\
             const short = key === 'short';\n\
             const length = short ? 1 : 2;\n\
             return Promise.resolve({ status: 206, bytes: new Uint8Array(length), contentRange: valid || short ? 'bytes 2-3/4' : 'bytes 0-1/4', contentLength: length });",
        );
        let store = JsStore::new(get);
        for name in ["valid", "wrong", "short"] {
            let key = StoreKey::new(name).map_err(|_| JsValue::from_str("invalid test key"))?;
            let result = store
                .get_partial(&key, ByteRange::FromStart(2, Some(2)))
                .await;
            if name == "valid" {
                let bytes = result
                    .map_err(|error| JsValue::from_str(&error.to_string()))?
                    .ok_or_else(|| JsValue::from_str("valid range became sparse"))?;
                assert_eq!(bytes.len(), 2);
            } else {
                assert!(result.is_err(), "{name} response was accepted");
            }
        }
        Ok(())
    }
}
