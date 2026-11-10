//! 纸条身份（v11 §1.4）。
//!
//! ID 必须是 **UUIDv7**，且文本形式只接受**小写连字符**规范形式。
//! 故意不接受大写、无连字符、带花括号或 URN 形式：文件格式只认一种写法，
//! 否则同一 ID 会有多种字符串表示，往返与去重都失去确定性。

use uuid::Uuid;

use crate::error::DomainError;

/// 纸条身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlipId(Uuid);

impl SlipId {
    /// 生成新的 UUIDv7。时间有序，适合追加式日志文件。
    pub fn new_v7() -> Self {
        Self(Uuid::now_v7())
    }

    /// 从已有 UUID 构造，并校验"非 nil + 版本 7"。
    pub fn from_uuid(uuid: Uuid) -> Result<Self, DomainError> {
        let id = Self(uuid);
        id.validate()?;
        Ok(id)
    }

    /// 从文件文本严格解析。
    ///
    /// 只接受 `Uuid::hyphenated()` 的小写输出。任何其他写法都返回
    /// [`DomainError::InvalidId`]，不做大小写或分隔符的宽容处理。
    pub fn parse_strict(text: &str) -> Result<Self, DomainError> {
        let uuid = Uuid::parse_str(text).map_err(|_| DomainError::InvalidId)?;
        // `hyphenated()` 输出固定小写连字符形式；与原文不同即不是规范形式。
        if uuid.hyphenated().to_string() != text {
            return Err(DomainError::InvalidId);
        }
        Self::from_uuid(uuid)
    }

    /// 校验"非 nil + 版本 7"。
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.0.is_nil() || self.0.get_version_num() != 7 {
            return Err(DomainError::InvalidId);
        }
        Ok(())
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// 文件里使用的唯一文本形式。
    pub fn to_canonical_string(&self) -> String {
        self.0.hyphenated().to_string()
    }
}

// 序列化只走规范形式：同一 ID 只有一种字符串表示，往返与去重才有确定性。
impl serde::Serialize for SlipId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_canonical_string())
    }
}

impl<'de> serde::Deserialize<'de> for SlipId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Self::parse_strict(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个"版本位 = 4"的 UUID，不引入 `v4` feature。
    fn non_v7_uuid() -> Uuid {
        let mut bytes = [0u8; 16];
        bytes[6] = 0x40; // 高 4 位是版本号
        bytes[8] = 0x80; // RFC 4122 variant
        Uuid::from_bytes(bytes)
    }

    #[test]
    fn new_v7_is_valid_and_round_trips() {
        let id = SlipId::new_v7();
        id.validate().unwrap();
        let text = id.to_canonical_string();
        assert_eq!(SlipId::parse_strict(&text).unwrap(), id);
    }

    #[test]
    fn canonical_form_is_lowercase_hyphenated() {
        let text = SlipId::new_v7().to_canonical_string();
        assert_eq!(text.len(), 36);
        assert!(
            text.chars()
                .all(|c| c.is_ascii_digit() || c == '-' || ('a'..='f').contains(&c))
        );
    }

    #[test]
    fn rejects_uppercase_form() {
        let text = SlipId::new_v7().to_canonical_string().to_uppercase();
        assert_eq!(SlipId::parse_strict(&text), Err(DomainError::InvalidId));
    }

    #[test]
    fn rejects_simple_form() {
        let text = SlipId::new_v7().to_canonical_string().replace('-', "");
        assert_eq!(SlipId::parse_strict(&text), Err(DomainError::InvalidId));
    }

    #[test]
    fn rejects_braced_form() {
        let text = format!("{{{}}}", SlipId::new_v7().to_canonical_string());
        assert_eq!(SlipId::parse_strict(&text), Err(DomainError::InvalidId));
    }

    #[test]
    fn rejects_non_v7() {
        // 版本 4 是随机 UUID，不满足"版本 7"。
        assert_eq!(
            SlipId::from_uuid(non_v7_uuid()),
            Err(DomainError::InvalidId)
        );
    }

    #[test]
    fn rejects_nil() {
        assert_eq!(SlipId::from_uuid(Uuid::nil()), Err(DomainError::InvalidId));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(
            SlipId::parse_strict("not-a-uuid"),
            Err(DomainError::InvalidId)
        );
        assert_eq!(SlipId::parse_strict(""), Err(DomainError::InvalidId));
    }

    #[test]
    fn ordering_follows_uuid_v7_time_prefix() {
        // 合法 v7：bytes[6] 高 4 位 = 7，bytes[8] 高 2 位 = 10。
        let a = SlipId(Uuid::from_u64_pair(
            0x019a_9f3c_2b70_7000,
            0x8000_0000_0000_0001,
        ));
        let b = SlipId(Uuid::from_u64_pair(
            0x019a_9f3c_2b70_7000,
            0x8000_0000_0000_0002,
        ));
        a.validate().unwrap();
        b.validate().unwrap();
        assert!(a < b);
    }
}
