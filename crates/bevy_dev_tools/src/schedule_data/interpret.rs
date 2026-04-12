//! Utilities for taking extracted [`ScheduleData`] and constructing convenient data structures for
//! that data.
//!
//! [`ScheduleData`] itself is intended to be an easy-to-serialize format. This however means it can
//! be in a form that isn't easily "queryable" or "walkable" (some parts are easier than others).
//! This module provides useful tools to construct data structures that *are* queryable/walkable.

use crate::schedule_data::serde::ScheduleData;

pub fn 
