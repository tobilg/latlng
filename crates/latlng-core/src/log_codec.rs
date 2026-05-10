use super::*;

pub(crate) fn encode_log_record(record: &LogRecord) -> Result<Vec<u8>> {
    serde_json::to_vec(record).map_err(|error| CoreError::Codec(error.to_string()))
}

pub(crate) fn decode_log_record(payload: &[u8]) -> Result<LogRecord> {
    if let Ok(record) = serde_json::from_slice::<LogRecord>(payload) {
        return Ok(record);
    }
    if let Ok(record) = bincode::deserialize::<LogRecord>(payload) {
        return Ok(record);
    }
    if let Ok(command) = serde_json::from_slice::<Command>(payload) {
        return Ok(LogRecord::Command(command));
    }
    let command = bincode::deserialize::<Command>(payload)
        .map_err(|error| CoreError::Codec(error.to_string()))?;
    Ok(LogRecord::Command(command))
}
