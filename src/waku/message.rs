use crate::{
    errors::NodeResult,
    utils::{crypto::sha256hash, get_current_time_nanos},
};

use base64::{prelude::BASE64_STANDARD, Engine};
use core::fmt;
use ecies::PublicKey;
use serde::{Deserialize, Serialize};

/// Within Waku Message and Content Topic we specify version to be 0 since
///  encryption takes place at our application layer, instead of at protocol layer of Waku.
pub const WAKU_ENC_VERSION: u8 = 0;

/// Within Content Topic we specify encoding to be `proto` as is the recommendation by Waku.
pub const WAKU_ENCODING: &str = "proto";

/// App-name for the Content Topic.
pub const WAKU_APP_NAME: &str = "dria";

/// We want messages to be short-lived, and furthermore a message response
/// only makes sense if it is responded to in a short time.
/// So it makes sense to have messages be ephemeral.
pub const WAKU_EPHEMERAL: bool = true;

/// A Waku message, as defined by [14/WAKU2-MESSAGE](https://github.com/vacp2p/rfc-index/blob/main/waku/standards/core/14/message.md).
///
/// ## Fields
///
/// - `payload`: The message payload as a base64 encoded data string.
/// - `content_topic`: The message content topic for optional content-based filtering.
/// - `version`: Message version. Used to indicate type of payload encryption. Default version is 0 (no payload encryption).
/// - `timestamp`: The time at which the message is generated by its sender. This field holds the Unix epoch time in nanoseconds as a 64-bits integer value.
/// - `ephemeral`: This flag indicates the transient nature of the message. Indicates if the message is eligible to be stored by the STORE protocol.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WakuMessage {
    pub payload: String,
    pub content_topic: String,
    #[serde(default)]
    pub version: u8,
    #[serde(default)]
    pub timestamp: u128,
    #[serde(default)]
    #[serde(skip_serializing)] // see: https://github.com/waku-org/nwaku/issues/2643
    pub ephemeral: bool,
}

/// 65-byte signature as hex characters take up 130 characters.
/// The 65-byte signature is composed of 64-byte RSV signature and 1-byte recovery id.
///
/// When recovery is not required and only verification is being done, we omit the recovery id
/// and therefore use 128 characters: SIGNATURE_SIZE - 2.
const SIGNATURE_SIZE: usize = 130;

impl WakuMessage {
    /// Creates a new ephemeral Waku message with current timestamp, version 0.
    ///
    /// ## Parameters
    ///
    /// - `payload` is gives as bytes. It is base64 encoded internally.
    /// - `topic` is the name of the topic itself within the full content topic. The rest of the content topic
    /// is filled in automatically, e.g. `/dria/0/<topic>/proto`.
    pub fn new(payload: impl AsRef<[u8]>, topic: &str) -> Self {
        WakuMessage {
            payload: BASE64_STANDARD.encode(payload),
            content_topic: Self::create_content_topic(topic).to_string(),
            version: WAKU_ENC_VERSION,
            timestamp: get_current_time_nanos(),
            ephemeral: WAKU_EPHEMERAL,
        }
    }

    /// Decodes the base64 payload into bytes.
    pub fn decode_payload(&self) -> Result<Vec<u8>, base64::DecodeError> {
        BASE64_STANDARD.decode(&self.payload)
    }

    /// Decodes and parses the payload into JSON.
    pub fn parse_payload<T: for<'a> Deserialize<'a>>(&self, signed: bool) -> NodeResult<T> {
        let payload = self.decode_payload()?;

        let body = if signed {
            // skips the 65 byte hex signature
            &payload[SIGNATURE_SIZE..]
        } else {
            &payload[..]
        };

        let parsed: T = serde_json::from_slice(body)?;
        Ok(parsed)
    }

    pub fn is_signed(&self, public_key: &PublicKey) -> NodeResult<bool> {
        // decode base64 payload
        let payload = self.decode_payload()?;

        // parse signature (64 bytes = 128 hex chars, although the full 65-byte RSV signature is given)
        let (signature, body) = (&payload[..SIGNATURE_SIZE - 2], &payload[SIGNATURE_SIZE..]);
        let signature = hex::decode(signature).expect("could not decode");
        let signature =
            libsecp256k1::Signature::parse_standard_slice(&signature).expect("could not parse");

        // verify signature
        let digest = libsecp256k1::Message::parse(&sha256hash(body));
        Ok(libsecp256k1::verify(&digest, &signature, public_key))
    }

    /// A [Content Topic](https://docs.waku.org/learn/concepts/content-topics) is represented as a string with the form:
    ///
    /// ```sh
    /// /app-name/version/content-topic/encoding
    /// /waku/2/default-waku/proto # example
    /// ```
    ///
    /// `app-name` defaults to `dria` unless specified otherwise with the second argument.
    #[inline]
    pub fn create_content_topic(topic: &str) -> String {
        format!(
            "/{}/{}/{}/{}",
            WAKU_APP_NAME, WAKU_ENC_VERSION, topic, WAKU_ENCODING
        )
    }
}

impl fmt::Display for WakuMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let payload_decoded = self
            .decode_payload()
            .unwrap_or(self.payload.as_bytes().to_vec());

        let payload_str = String::from_utf8(payload_decoded).unwrap_or(self.payload.clone());
        write!(
            f,
            "WakuMessage {} at {}\n{}",
            self.content_topic, self.timestamp, payload_str
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsecp256k1::{Message, SecretKey};
    use rand::thread_rng;
    use serde_json::json;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct TestStruct {
        hello: String,
    }

    impl Default for TestStruct {
        fn default() -> Self {
            TestStruct {
                hello: "world".to_string(),
            }
        }
    }

    const TOPIC: &str = "test-topic";

    #[test]
    fn test_create_content_topic() {
        let expected = "/dria/0/test-topic/proto".to_string();
        assert_eq!(WakuMessage::create_content_topic(TOPIC), expected);
    }

    #[test]
    fn test_display_message() {
        let message = WakuMessage::new(b"hello world", "test-topic");
        println!("{}", message);
    }

    #[test]
    fn test_unsigned_message() {
        // create payload & message
        let body = TestStruct::default();
        let payload = serde_json::to_vec(&json!(body)).expect("Should serialize");
        let message = WakuMessage::new(payload, TOPIC);

        // decode message
        let message_body = message.decode_payload().expect("Should decode");
        let body = serde_json::from_slice::<TestStruct>(&message_body).expect("Should deserialize");
        assert_eq!(
            serde_json::to_string(&body).expect("Should stringify"),
            "{\"hello\":\"world\"}"
        );
        assert_eq!(message.content_topic, "/dria/0/test-topic/proto");
        assert_eq!(message.version, WAKU_ENC_VERSION);
        assert_eq!(message.ephemeral, true);
        assert!(message.timestamp > 0);

        let parsed_body = message.parse_payload(false).expect("Should decode");
        assert_eq!(body, parsed_body);
    }

    #[test]
    fn test_signed_message() {
        let mut rng = thread_rng();
        let sk = SecretKey::random(&mut rng);

        // create payload & message with signature & body
        let body = TestStruct::default();
        let body_str = serde_json::to_string(&json!(body)).expect("Should stringify");
        let (signature, recid) = libsecp256k1::sign(&Message::parse(&sha256hash(&body_str)), &sk);
        let signature_str = format!(
            "{}{}",
            hex::encode(signature.serialize()),
            hex::encode([recid.serialize()])
        );
        let payload = format!("{}{}", signature_str, body_str);
        let message = WakuMessage::new(payload, TOPIC);

        // decode message
        let message_body = message.decode_payload().expect("Should decode");
        let body =
            serde_json::from_slice::<TestStruct>(&message_body[130..]).expect("Should parse");
        assert_eq!(
            serde_json::to_string(&body).expect("Should stringify"),
            "{\"hello\":\"world\"}"
        );
        assert_eq!(message.content_topic, "/dria/0/test-topic/proto");
        assert_eq!(message.version, WAKU_ENC_VERSION);
        assert_eq!(message.ephemeral, true);
        assert!(message.timestamp > 0);

        // check signature
        let pk = PublicKey::from_secret_key(&sk);
        assert!(message.is_signed(&pk).expect("Should check signature"));

        let parsed_body = message.parse_payload(true).expect("Should decode");
        assert_eq!(body, parsed_body);
    }
}
