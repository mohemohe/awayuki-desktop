use serde::Serialize;

use super::identity::FederationProtocol;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusOperation {
    Favourite,
    Unfavourite,
    Reblog,
    Unreblog,
    Bookmark,
    Unbookmark,
    Vote,
    Edit,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationshipOperation {
    Follow,
    Unfollow,
    Mute,
    Unmute,
    Block,
    Unblock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineOperation {
    Home,
    Public,
    Local,
    Lists,
    Hashtags,
    Notifications,
    Bookmarks,
    Favourites,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineCapabilities {
    pub home: bool,
    pub public: bool,
    pub local: bool,
    pub lists: bool,
    pub hashtags: bool,
    pub notifications: bool,
    pub bookmarks: bool,
    pub favourites: bool,
}

impl TimelineCapabilities {
    pub fn supports(&self, operation: TimelineOperation) -> bool {
        match operation {
            TimelineOperation::Home => self.home,
            TimelineOperation::Public => self.public,
            TimelineOperation::Local => self.local,
            TimelineOperation::Lists => self.lists,
            TimelineOperation::Hashtags => self.hashtags,
            TimelineOperation::Notifications => self.notifications,
            TimelineOperation::Bookmarks => self.bookmarks,
            TimelineOperation::Favourites => self.favourites,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusCapabilities {
    pub favourite: bool,
    pub reblog: bool,
    pub bookmark: bool,
    pub vote: bool,
    pub edit: bool,
    pub delete: bool,
}

impl StatusCapabilities {
    pub fn supports(&self, operation: StatusOperation) -> bool {
        match operation {
            StatusOperation::Favourite | StatusOperation::Unfavourite => self.favourite,
            StatusOperation::Reblog | StatusOperation::Unreblog => self.reblog,
            StatusOperation::Bookmark | StatusOperation::Unbookmark => self.bookmark,
            StatusOperation::Vote => self.vote,
            StatusOperation::Edit => self.edit,
            StatusOperation::Delete => self.delete,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipCapabilities {
    pub follow: bool,
    pub mute: bool,
    pub block: bool,
}

impl RelationshipCapabilities {
    pub fn supports(&self, operation: RelationshipOperation) -> bool {
        match operation {
            RelationshipOperation::Follow | RelationshipOperation::Unfollow => self.follow,
            RelationshipOperation::Mute | RelationshipOperation::Unmute => self.mute,
            RelationshipOperation::Block | RelationshipOperation::Unblock => self.block,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ComposeCapabilities {
    pub media_upload: bool,
    pub poll: bool,
    pub quote: bool,
    pub max_media_attachments: u16,
    pub max_characters: u32,
}

/// Immutable feature negotiation snapshot attached to a login session.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionCapabilities {
    pub protocol: FederationProtocol,
    pub timelines: TimelineCapabilities,
    pub status: StatusCapabilities,
    pub relationship: RelationshipCapabilities,
    pub compose: ComposeCapabilities,
    pub streaming: bool,
}

impl SessionCapabilities {
    pub fn require_timeline(&self, operation: TimelineOperation) -> Result<(), CapabilityError> {
        self.timelines
            .supports(operation)
            .then_some(())
            .ok_or(CapabilityError::Timeline { operation })
    }

    pub fn require_status(&self, operation: StatusOperation) -> Result<(), CapabilityError> {
        self.status
            .supports(operation)
            .then_some(())
            .ok_or(CapabilityError::Status { operation })
    }

    pub fn require_relationship(
        &self,
        operation: RelationshipOperation,
    ) -> Result<(), CapabilityError> {
        self.relationship
            .supports(operation)
            .then_some(())
            .ok_or(CapabilityError::Relationship { operation })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityError {
    #[error("timeline operation is not supported: {operation:?}")]
    Timeline { operation: TimelineOperation },
    #[error("status operation is not supported: {operation:?}")]
    Status { operation: StatusOperation },
    #[error("relationship operation is not supported: {operation:?}")]
    Relationship { operation: RelationshipOperation },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_is_distinct_from_an_empty_result() {
        let capabilities = SessionCapabilities {
            protocol: FederationProtocol::AtProto,
            timelines: TimelineCapabilities {
                home: true,
                public: true,
                local: false,
                lists: false,
                hashtags: false,
                notifications: true,
                bookmarks: false,
                favourites: false,
            },
            status: StatusCapabilities {
                favourite: true,
                reblog: true,
                bookmark: false,
                vote: false,
                edit: true,
                delete: true,
            },
            relationship: RelationshipCapabilities {
                follow: true,
                mute: true,
                block: true,
            },
            compose: ComposeCapabilities {
                media_upload: true,
                poll: false,
                quote: true,
                max_media_attachments: 4,
                max_characters: 300,
            },
            streaming: true,
        };

        assert!(matches!(
            capabilities.require_status(StatusOperation::Vote),
            Err(CapabilityError::Status { .. })
        ));
    }
}
