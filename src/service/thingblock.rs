//! The thingblock service: thingblock-specific state the servers share, today just the served
//! resource root (see [`resource`]).

pub mod resource;

use resource::ResourceRoot;

/// Thingblock-specific state shared by every connection.
pub struct ThingblockService {
    resource_root: ResourceRoot,
}

impl ThingblockService {
    pub fn new(resource_root: ResourceRoot) -> Self {
        Self { resource_root }
    }

    pub fn resource_root(&self) -> &ResourceRoot {
        &self.resource_root
    }
}
