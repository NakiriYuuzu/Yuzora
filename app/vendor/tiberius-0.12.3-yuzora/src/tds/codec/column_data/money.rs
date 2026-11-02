use crate::{error::Error, sql_read_bytes::SqlReadBytes, tds::Numeric, ColumnData};

pub(crate) async fn decode<R>(src: &mut R, len: u8) -> crate::Result<ColumnData<'static>>
where
    R: SqlReadBytes + Unpin,
{
    let res = match len {
        0 => ColumnData::Numeric(None),
        4 => ColumnData::Numeric(Some(Numeric::new_with_scale(
            i128::from(src.read_i32_le().await?),
            4,
        ))),
        8 => {
            let high = i64::from(src.read_i32_le().await?);
            let low = i64::from(src.read_u32_le().await?);
            let raw = (high << 32) | low;
            ColumnData::Numeric(Some(Numeric::new_with_scale(i128::from(raw), 4)))
        }
        _ => {
            return Err(Error::Protocol(
                format!("money: length of {} is invalid", len).into(),
            ))
        }
    };

    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_read_bytes::test_utils::IntoSqlReadBytes;
    use bytes::{BufMut, BytesMut};

    async fn decode_smallmoney(raw: i32) -> Numeric {
        let mut bytes = BytesMut::new();
        bytes.put_i32_le(raw);
        let mut reader = bytes.into_sql_read_bytes();
        match decode(&mut reader, 4).await.unwrap() {
            ColumnData::Numeric(Some(value)) => value,
            other => panic!("expected exact smallmoney Numeric, got {other:?}"),
        }
    }

    async fn decode_money(raw: i64) -> Numeric {
        let mut bytes = BytesMut::new();
        bytes.put_i32_le((raw >> 32) as i32);
        bytes.put_u32_le(raw as u32);
        let mut reader = bytes.into_sql_read_bytes();
        match decode(&mut reader, 8).await.unwrap() {
            ColumnData::Numeric(Some(value)) => value,
            other => panic!("expected exact money Numeric, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn smallmoney_preserves_sign_boundaries_and_four_digit_scale() {
        for raw in [12_345, -12_345, i32::MAX, i32::MIN] {
            let value = decode_smallmoney(raw).await;
            assert_eq!(value.value(), i128::from(raw));
            assert_eq!(value.scale(), 4);
        }
    }

    #[tokio::test]
    async fn money_reassembles_signed_high_and_unsigned_low_without_f64() {
        for raw in [12_300, -12_300, i64::MAX, i64::MIN] {
            let value = decode_money(raw).await;
            assert_eq!(value.value(), i128::from(raw));
            assert_eq!(value.scale(), 4);
        }
    }

    #[tokio::test]
    async fn null_money_remains_null_without_fabricating_zero() {
        let mut reader = BytesMut::new().into_sql_read_bytes();
        assert_eq!(
            decode(&mut reader, 0).await.unwrap(),
            ColumnData::Numeric(None)
        );
    }
}
