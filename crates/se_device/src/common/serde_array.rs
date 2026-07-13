//! Serde support for fixed-size arrays larger than Serde's built-in implementations.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub(crate) fn serialize<T, S, const N: usize>(
    value: &[T; N],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    value.as_slice().serialize(serializer)
}

pub(crate) fn deserialize<'de, T, D, const N: usize>(deserializer: D) -> Result<[T; N], D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    let values = Vec::<T>::deserialize(deserializer)?;
    let length = values.len();
    values.try_into().map_err(|_| {
        serde::de::Error::invalid_length(length, &"a fixed-size array with the expected length")
    })
}
