//! Message type mapping for metrics labeling.
//!
//! Provides functions to convert SV2 message types to string labels for Prometheus metrics.

use super::msg_type;
use crate::stratum_core::{
    common_messages_sv2::{
        MESSAGE_TYPE_CHANNEL_ENDPOINT_CHANGED, MESSAGE_TYPE_RECONNECT,
        MESSAGE_TYPE_SETUP_CONNECTION, MESSAGE_TYPE_SETUP_CONNECTION_ERROR,
        MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
    },
    mining_sv2::{
        MESSAGE_TYPE_CLOSE_CHANNEL, MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH,
        MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB, MESSAGE_TYPE_NEW_MINING_JOB,
        MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
        MESSAGE_TYPE_OPEN_MINING_CHANNEL_ERROR, MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS, MESSAGE_TYPE_SET_CUSTOM_MINING_JOB,
        MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR, MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS,
        MESSAGE_TYPE_SET_EXTRANONCE_PREFIX, MESSAGE_TYPE_SET_GROUP_CHANNEL, MESSAGE_TYPE_SET_TARGET,
        MESSAGE_TYPE_SUBMIT_SHARES_ERROR, MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
        MESSAGE_TYPE_SUBMIT_SHARES_STANDARD, MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
        MESSAGE_TYPE_UPDATE_CHANNEL, MESSAGE_TYPE_UPDATE_CHANNEL_ERROR,
    },
    parsers_sv2::{Mining, TemplateDistribution},
    template_distribution_sv2::{
        MESSAGE_TYPE_NEW_TEMPLATE, MESSAGE_TYPE_REQUEST_TRANSACTION_DATA,
        MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_ERROR, MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
        MESSAGE_TYPE_SET_NEW_PREV_HASH, MESSAGE_TYPE_SUBMIT_SOLUTION,
    },
};
use crate::utils::protocol_message_type::{protocol_message_type, MessageType};

/// Map a Mining message enum to its metric label string.
pub fn mining_msg_type(msg: &Mining<'_>) -> &'static str {
    match msg {
        Mining::OpenStandardMiningChannel(_) => msg_type::OPEN_STANDARD_MINING_CHANNEL,
        Mining::OpenStandardMiningChannelSuccess(_) => msg_type::OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
        Mining::OpenExtendedMiningChannel(_) => msg_type::OPEN_EXTENDED_MINING_CHANNEL,
        Mining::OpenExtendedMiningChannelSuccess(_) => msg_type::OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
        Mining::OpenMiningChannelError(_) => msg_type::OPEN_MINING_CHANNEL_ERROR,
        Mining::UpdateChannel(_) => msg_type::UPDATE_CHANNEL,
        Mining::UpdateChannelError(_) => msg_type::UPDATE_CHANNEL_ERROR,
        Mining::CloseChannel(_) => msg_type::CLOSE_CHANNEL,
        Mining::SetExtranoncePrefix(_) => msg_type::SET_EXTRANONCE_PREFIX,
        Mining::SubmitSharesStandard(_) => msg_type::SUBMIT_SHARES_STANDARD,
        Mining::SubmitSharesExtended(_) => msg_type::SUBMIT_SHARES_EXTENDED,
        Mining::SubmitSharesSuccess(_) => msg_type::SUBMIT_SHARES_SUCCESS,
        Mining::SubmitSharesError(_) => msg_type::SUBMIT_SHARES_ERROR,
        Mining::NewMiningJob(_) => msg_type::NEW_MINING_JOB,
        Mining::NewExtendedMiningJob(_) => msg_type::NEW_EXTENDED_MINING_JOB,
        Mining::SetNewPrevHash(_) => msg_type::SET_NEW_PREV_HASH,
        Mining::SetTarget(_) => msg_type::SET_TARGET,
        Mining::SetCustomMiningJob(_) => msg_type::SET_CUSTOM_MINING_JOB,
        Mining::SetCustomMiningJobSuccess(_) => msg_type::SET_CUSTOM_MINING_JOB_SUCCESS,
        Mining::SetCustomMiningJobError(_) => msg_type::SET_CUSTOM_MINING_JOB_ERROR,
        Mining::SetGroupChannel(_) => msg_type::SET_GROUP_CHANNEL,
    }
}

/// Map a TemplateDistribution message enum to its metric label string.
pub fn template_distribution_msg_type(msg: &TemplateDistribution<'_>) -> &'static str {
    match msg {
        TemplateDistribution::NewTemplate(_) => msg_type::NEW_TEMPLATE,
        TemplateDistribution::SetNewPrevHash(_) => msg_type::SET_NEW_PREV_HASH_TP,
        TemplateDistribution::SubmitSolution(_) => msg_type::SUBMIT_SOLUTION,
        TemplateDistribution::RequestTransactionData(_) => msg_type::REQUEST_TRANSACTION_DATA,
        TemplateDistribution::RequestTransactionDataSuccess(_) => msg_type::REQUEST_TRANSACTION_DATA_SUCCESS,
        TemplateDistribution::RequestTransactionDataError(_) => msg_type::REQUEST_TRANSACTION_DATA_ERROR,
        _ => msg_type::UNKNOWN,
    }
}

/// Map raw header ext_type and msg_type to a metric label string.
///
/// Used when only the frame header is available (e.g., in translator upstream).
pub fn header_to_msg_type(ext_type: u16, msg_type_byte: u8) -> &'static str {
    match protocol_message_type(ext_type, msg_type_byte) {
        MessageType::Common => common_msg_type(msg_type_byte),
        MessageType::Mining => mining_msg_type_from_byte(msg_type_byte),
        MessageType::TemplateDistribution => template_distribution_msg_type_from_byte(msg_type_byte),
        _ => msg_type::UNKNOWN,
    }
}

fn common_msg_type(msg_type_byte: u8) -> &'static str {
    match msg_type_byte {
        MESSAGE_TYPE_SETUP_CONNECTION => msg_type::SETUP_CONNECTION,
        MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS => msg_type::SETUP_CONNECTION_SUCCESS,
        MESSAGE_TYPE_SETUP_CONNECTION_ERROR => msg_type::SETUP_CONNECTION_ERROR,
        MESSAGE_TYPE_CHANNEL_ENDPOINT_CHANGED => msg_type::CHANNEL_ENDPOINT_CHANGED,
        MESSAGE_TYPE_RECONNECT => msg_type::RECONNECT,
        _ => msg_type::UNKNOWN,
    }
}

fn mining_msg_type_from_byte(msg_type_byte: u8) -> &'static str {
    match msg_type_byte {
        MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL => msg_type::OPEN_STANDARD_MINING_CHANNEL,
        MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS => msg_type::OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
        MESSAGE_TYPE_OPEN_MINING_CHANNEL_ERROR => msg_type::OPEN_MINING_CHANNEL_ERROR,
        MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL => msg_type::OPEN_EXTENDED_MINING_CHANNEL,
        MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS => msg_type::OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
        MESSAGE_TYPE_NEW_MINING_JOB => msg_type::NEW_MINING_JOB,
        MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB => msg_type::NEW_EXTENDED_MINING_JOB,
        MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH => msg_type::SET_NEW_PREV_HASH,
        MESSAGE_TYPE_SET_TARGET => msg_type::SET_TARGET,
        MESSAGE_TYPE_SET_EXTRANONCE_PREFIX => msg_type::SET_EXTRANONCE_PREFIX,
        MESSAGE_TYPE_SUBMIT_SHARES_STANDARD => msg_type::SUBMIT_SHARES_STANDARD,
        MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED => msg_type::SUBMIT_SHARES_EXTENDED,
        MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS => msg_type::SUBMIT_SHARES_SUCCESS,
        MESSAGE_TYPE_SUBMIT_SHARES_ERROR => msg_type::SUBMIT_SHARES_ERROR,
        MESSAGE_TYPE_UPDATE_CHANNEL => msg_type::UPDATE_CHANNEL,
        MESSAGE_TYPE_UPDATE_CHANNEL_ERROR => msg_type::UPDATE_CHANNEL_ERROR,
        MESSAGE_TYPE_CLOSE_CHANNEL => msg_type::CLOSE_CHANNEL,
        MESSAGE_TYPE_SET_CUSTOM_MINING_JOB => msg_type::SET_CUSTOM_MINING_JOB,
        MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS => msg_type::SET_CUSTOM_MINING_JOB_SUCCESS,
        MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR => msg_type::SET_CUSTOM_MINING_JOB_ERROR,
        MESSAGE_TYPE_SET_GROUP_CHANNEL => msg_type::SET_GROUP_CHANNEL,
        _ => msg_type::UNKNOWN,
    }
}

fn template_distribution_msg_type_from_byte(msg_type_byte: u8) -> &'static str {
    match msg_type_byte {
        MESSAGE_TYPE_NEW_TEMPLATE => msg_type::NEW_TEMPLATE,
        MESSAGE_TYPE_SET_NEW_PREV_HASH => msg_type::SET_NEW_PREV_HASH_TP,
        MESSAGE_TYPE_SUBMIT_SOLUTION => msg_type::SUBMIT_SOLUTION,
        MESSAGE_TYPE_REQUEST_TRANSACTION_DATA => msg_type::REQUEST_TRANSACTION_DATA,
        MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS => msg_type::REQUEST_TRANSACTION_DATA_SUCCESS,
        MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_ERROR => msg_type::REQUEST_TRANSACTION_DATA_ERROR,
        _ => msg_type::UNKNOWN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mining_msg_type_from_byte() {
        assert_eq!(mining_msg_type_from_byte(MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL), msg_type::OPEN_STANDARD_MINING_CHANNEL);
        assert_eq!(mining_msg_type_from_byte(MESSAGE_TYPE_SUBMIT_SHARES_STANDARD), msg_type::SUBMIT_SHARES_STANDARD);
        assert_eq!(mining_msg_type_from_byte(MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB), msg_type::NEW_EXTENDED_MINING_JOB);
        assert_eq!(mining_msg_type_from_byte(255), msg_type::UNKNOWN);
    }

    #[test]
    fn test_common_msg_type() {
        assert_eq!(common_msg_type(MESSAGE_TYPE_SETUP_CONNECTION), msg_type::SETUP_CONNECTION);
        assert_eq!(common_msg_type(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS), msg_type::SETUP_CONNECTION_SUCCESS);
        assert_eq!(common_msg_type(255), msg_type::UNKNOWN);
    }

    #[test]
    fn test_template_distribution_msg_type_from_byte() {
        assert_eq!(template_distribution_msg_type_from_byte(MESSAGE_TYPE_NEW_TEMPLATE), msg_type::NEW_TEMPLATE);
        assert_eq!(template_distribution_msg_type_from_byte(MESSAGE_TYPE_SUBMIT_SOLUTION), msg_type::SUBMIT_SOLUTION);
        assert_eq!(template_distribution_msg_type_from_byte(255), msg_type::UNKNOWN);
    }

    #[test]
    fn test_header_to_msg_type_common() {
        // ext_type 0 = common messages
        assert_eq!(header_to_msg_type(0, MESSAGE_TYPE_SETUP_CONNECTION), msg_type::SETUP_CONNECTION);
    }

    #[test]
    fn test_header_to_msg_type_mining() {
        // ext_type 0 with mining message types
        assert_eq!(header_to_msg_type(0, MESSAGE_TYPE_SUBMIT_SHARES_STANDARD), msg_type::SUBMIT_SHARES_STANDARD);
    }
}
