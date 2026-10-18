//! Sidebar order of workspaces and workspace groups.
//!
//! The sidebar holds top-level items: ungrouped workspaces and groups, each
//! group with its own ordered members. Favorites are never grouped and always
//! come first. This module owns that order and its rules; the GTK rows only
//! mirror it, and `AppState::workspaces` follows `workspace_order`.

use std::collections::HashSet;

use crate::layout_state::WorkspaceGroupState;
use crate::workspace_color::WorkspaceColor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceGroup {
    pub id: String,
    pub name: String,
    pub color: WorkspaceColor,
    pub collapsed: bool,
    members: Vec<String>,
}

impl WorkspaceGroup {
    pub fn members(&self) -> &[String] {
        &self.members
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Workspace { id: String, favorite: bool },
    Group(WorkspaceGroup),
}

/// One sidebar row, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarRow<'a> {
    GroupHeader(&'a WorkspaceGroup),
    Workspace {
        id: &'a str,
        group: Option<&'a WorkspaceGroup>,
    },
}

impl SidebarRow<'_> {
    /// Whether the row is shown: a collapsed group hides its members, except
    /// the active workspace until the user switches away from it.
    pub fn visible(&self, active_workspace_id: Option<&str>) -> bool {
        match self {
            Self::GroupHeader(_) => true,
            Self::Workspace { id, group } => {
                group.is_none_or(|group| !group.collapsed) || active_workspace_id == Some(*id)
            }
        }
    }
}

/// A row something is dropped on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowRef {
    Workspace(String),
    Group(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Before,
    After,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GroupError {
    UnknownGroup,
    UnknownWorkspace,
    EmptyName,
    /// A group with members cannot be deleted; carries the member count.
    NotEmpty(usize),
}

impl std::fmt::Display for GroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownGroup => f.write_str("group not found"),
            Self::UnknownWorkspace => f.write_str("workspace not found"),
            Self::EmptyName => f.write_str("group name must not be empty"),
            Self::NotEmpty(count) => write!(
                f,
                "group still has {count} workspace{}; move or close {} first",
                if *count == 1 { "" } else { "s" },
                if *count == 1 { "it" } else { "them" },
            ),
        }
    }
}

/// Where a workspace sits: a top-level index, or a group's top-level index
/// and the member index inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Location {
    TopLevel(usize),
    InGroup(usize, usize),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SidebarLayout {
    items: Vec<Item>,
}

impl SidebarLayout {
    /// Rebuild the layout from a saved session. Members of one group are
    /// gathered at the group's first member; an empty group goes back to its
    /// saved position. References to unknown groups are dropped.
    pub fn from_saved<'a>(
        workspaces: impl IntoIterator<Item = (String, bool, Option<&'a str>)>,
        groups: &[WorkspaceGroupState],
    ) -> Self {
        let mut layout = Self::default();
        let mut seen = HashSet::new();
        let saved: Vec<&WorkspaceGroupState> = groups
            .iter()
            .filter(|group| seen.insert(group.id.as_str()))
            .collect();
        let mut placed = HashSet::new();
        for (id, favorite, group_id) in workspaces {
            let group = group_id
                .filter(|_| !favorite)
                .and_then(|group_id| saved.iter().find(|group| group.id == group_id));
            let Some(group) = group else {
                layout.items.push(Item::Workspace { id, favorite });
                continue;
            };
            if placed.insert(group.id.as_str()) {
                let restored = layout.restored_group(group);
                layout.items.push(Item::Group(restored));
            }
            if let Some(Item::Group(target)) = layout
                .items
                .iter_mut()
                .find(|item| matches!(item, Item::Group(g) if g.id == group.id))
            {
                target.members.push(id);
            }
        }
        let mut empty: Vec<&WorkspaceGroupState> = saved
            .into_iter()
            .filter(|group| !placed.contains(group.id.as_str()))
            .collect();
        empty.sort_by_key(|group| group.position);
        for group in empty {
            let restored = layout.restored_group(group);
            let index = group.position.min(layout.items.len());
            layout.items.insert(index, Item::Group(restored));
        }
        layout.settle();
        layout
    }

    fn restored_group(&self, group: &WorkspaceGroupState) -> WorkspaceGroup {
        WorkspaceGroup {
            id: group.id.clone(),
            name: group.name.clone(),
            color: group.color.unwrap_or_else(|| self.next_group_color()),
            collapsed: group.collapsed,
            members: Vec::new(),
        }
    }

    /// The groups to save, with their top-level positions.
    pub fn saved_groups(&self) -> Vec<WorkspaceGroupState> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(position, item)| match item {
                Item::Group(group) => Some(WorkspaceGroupState {
                    id: group.id.clone(),
                    name: group.name.clone(),
                    color: Some(group.color),
                    collapsed: group.collapsed,
                    position,
                }),
                Item::Workspace { .. } => None,
            })
            .collect()
    }

    /// Add a workspace at the end, or after the other favorites.
    pub fn push_workspace(&mut self, id: impl Into<String>, favorite: bool) {
        self.items.push(Item::Workspace {
            id: id.into(),
            favorite,
        });
        self.settle();
    }

    /// Remove a workspace. Its group stays, even when left empty.
    pub fn remove_workspace(&mut self, id: &str) -> bool {
        self.take_workspace(id).is_some()
    }

    /// Workspace ids in sidebar order, including collapsed members.
    pub fn workspace_order(&self) -> Vec<String> {
        self.rows()
            .into_iter()
            .filter_map(|row| match row {
                SidebarRow::Workspace { id, .. } => Some(id.to_string()),
                SidebarRow::GroupHeader(_) => None,
            })
            .collect()
    }

    pub fn rows(&self) -> Vec<SidebarRow<'_>> {
        let mut rows = Vec::new();
        for item in &self.items {
            match item {
                Item::Workspace { id, .. } => rows.push(SidebarRow::Workspace { id, group: None }),
                Item::Group(group) => {
                    rows.push(SidebarRow::GroupHeader(group));
                    rows.extend(group.members.iter().map(|id| SidebarRow::Workspace {
                        id,
                        group: Some(group),
                    }));
                }
            }
        }
        rows
    }

    pub fn groups(&self) -> impl Iterator<Item = &WorkspaceGroup> {
        self.items.iter().filter_map(|item| match item {
            Item::Group(group) => Some(group),
            Item::Workspace { .. } => None,
        })
    }

    pub fn group(&self, id: &str) -> Option<&WorkspaceGroup> {
        self.groups().find(|group| group.id == id)
    }

    pub fn group_of(&self, workspace_id: &str) -> Option<&WorkspaceGroup> {
        match self.locate(workspace_id)? {
            Location::InGroup(index, _) => match &self.items[index] {
                Item::Group(group) => Some(group),
                Item::Workspace { .. } => None,
            },
            Location::TopLevel(_) => None,
        }
    }

    pub fn is_favorite(&self, workspace_id: &str) -> bool {
        self.items.iter().any(
            |item| matches!(item, Item::Workspace { id, favorite: true } if id == workspace_id),
        )
    }

    /// Pin or unpin a workspace. Pinning takes it out of its group; either
    /// way it lands at the boundary between favorites and the rest.
    pub fn set_favorite(&mut self, workspace_id: &str, favorite: bool) -> bool {
        if self.take_workspace(workspace_id).is_none() {
            return false;
        }
        let index = self.favorites_len();
        self.items.insert(
            index,
            Item::Workspace {
                id: workspace_id.to_string(),
                favorite,
            },
        );
        true
    }

    /// Create a group holding `members`, placed where its first member was.
    /// A missing name or colour gets the next default.
    pub fn create_group(
        &mut self,
        name: Option<&str>,
        color: Option<WorkspaceColor>,
        members: &[String],
    ) -> Result<String, GroupError> {
        if members.iter().any(|id| self.locate(id).is_none()) {
            return Err(GroupError::UnknownWorkspace);
        }
        let name = match name.map(str::trim) {
            Some("") => return Err(GroupError::EmptyName),
            Some(name) => name.to_string(),
            None => self.next_group_name(),
        };
        let id = uuid::Uuid::new_v4().to_string();
        let index = match members.first().and_then(|first| self.locate(first)) {
            Some(Location::TopLevel(index)) => index,
            Some(Location::InGroup(index, _)) => index + 1,
            None => self.items.len(),
        };
        self.items.insert(
            index,
            Item::Group(WorkspaceGroup {
                id: id.clone(),
                name,
                color: color.unwrap_or_else(|| self.next_group_color()),
                collapsed: false,
                members: Vec::new(),
            }),
        );
        for member in members {
            if self.take_workspace(member).is_some() {
                self.group_mut(&id)
                    .expect("group just inserted")
                    .members
                    .push(member.clone());
            }
        }
        self.settle();
        Ok(id)
    }

    pub fn rename_group(&mut self, id: &str, name: &str) -> Result<(), GroupError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(GroupError::EmptyName);
        }
        self.group_mut(id).ok_or(GroupError::UnknownGroup)?.name = name.to_string();
        Ok(())
    }

    pub fn set_group_color(&mut self, id: &str, color: WorkspaceColor) -> Result<(), GroupError> {
        self.group_mut(id).ok_or(GroupError::UnknownGroup)?.color = color;
        Ok(())
    }

    pub fn set_group_collapsed(&mut self, id: &str, collapsed: bool) -> Result<(), GroupError> {
        self.group_mut(id)
            .ok_or(GroupError::UnknownGroup)?
            .collapsed = collapsed;
        Ok(())
    }

    /// Delete an empty group. A group with members is refused.
    pub fn delete_group(&mut self, id: &str) -> Result<(), GroupError> {
        let index = self.group_index(id).ok_or(GroupError::UnknownGroup)?;
        if let Item::Group(group) = &self.items[index] {
            if !group.members.is_empty() {
                return Err(GroupError::NotEmpty(group.members.len()));
            }
        }
        self.items.remove(index);
        Ok(())
    }

    /// Move a workspace to the end of a group, or out of its group to just
    /// after it. Grouping a favorite unpins it.
    pub fn set_workspace_group(
        &mut self,
        workspace_id: &str,
        group_id: Option<&str>,
    ) -> Result<(), GroupError> {
        let location = self
            .locate(workspace_id)
            .ok_or(GroupError::UnknownWorkspace)?;
        match group_id {
            Some(group_id) => {
                if self.group(group_id).is_none() {
                    return Err(GroupError::UnknownGroup);
                }
                if self
                    .group_of(workspace_id)
                    .is_some_and(|g| g.id == group_id)
                {
                    return Ok(());
                }
                self.take_workspace(workspace_id);
                self.group_mut(group_id)
                    .expect("group checked above")
                    .members
                    .push(workspace_id.to_string());
            }
            None => {
                let Location::InGroup(index, _) = location else {
                    return Ok(());
                };
                self.take_workspace(workspace_id);
                self.items.insert(
                    index + 1,
                    Item::Workspace {
                        id: workspace_id.to_string(),
                        favorite: false,
                    },
                );
            }
        }
        self.settle();
        Ok(())
    }

    /// Move a workspace next to a row. Next to a group member it joins that
    /// group; above a group header it lands outside the group, below one it
    /// becomes the group's first member. Favorites stay above the rest.
    pub fn move_workspace(
        &mut self,
        workspace_id: &str,
        target: &RowRef,
        placement: Placement,
    ) -> bool {
        if target == &RowRef::Workspace(workspace_id.to_string()) || !self.contains(target) {
            return false;
        }
        let Some(favorite) = self.take_workspace(workspace_id) else {
            return false;
        };
        let id = workspace_id.to_string();
        let after = usize::from(placement == Placement::After);
        match target {
            RowRef::Group(group_id) => {
                let index = self.group_index(group_id).expect("target checked above");
                match placement {
                    Placement::Before => self.insert_top_level(index, id, favorite),
                    Placement::After => self
                        .group_mut(group_id)
                        .expect("target checked above")
                        .members
                        .insert(0, id),
                }
            }
            RowRef::Workspace(target_id) => {
                match self.locate(target_id).expect("target checked above") {
                    Location::TopLevel(index) => self.insert_top_level(index + after, id, favorite),
                    Location::InGroup(index, member) => {
                        if let Item::Group(group) = &mut self.items[index] {
                            group.members.insert(member + after, id);
                        }
                    }
                }
            }
        }
        true
    }

    /// Move a whole group next to a top-level item. A member row stands for
    /// its group, so groups never nest.
    pub fn move_group(&mut self, group_id: &str, target: &RowRef, placement: Placement) -> bool {
        let target_index = match target {
            RowRef::Group(id) => self.group_index(id),
            RowRef::Workspace(id) => self.locate(id).map(|location| match location {
                Location::TopLevel(index) | Location::InGroup(index, _) => index,
            }),
        };
        let (Some(source), Some(target_index)) = (self.group_index(group_id), target_index) else {
            return false;
        };
        if source == target_index {
            return false;
        }
        let group = self.items.remove(source);
        let target_index = if target_index > source {
            target_index - 1
        } else {
            target_index
        };
        let index = target_index + usize::from(placement == Placement::After);
        let index = index.max(self.favorites_len());
        self.items.insert(index, group);
        true
    }

    fn contains(&self, target: &RowRef) -> bool {
        match target {
            RowRef::Workspace(id) => self.locate(id).is_some(),
            RowRef::Group(id) => self.group_index(id).is_some(),
        }
    }

    fn insert_top_level(&mut self, index: usize, id: String, favorite: bool) {
        let favorites = self.favorites_len();
        let index = if favorite {
            index.min(favorites)
        } else {
            index.max(favorites)
        };
        self.items.insert(index, Item::Workspace { id, favorite });
    }

    fn locate(&self, workspace_id: &str) -> Option<Location> {
        self.items
            .iter()
            .enumerate()
            .find_map(|(index, item)| match item {
                Item::Workspace { id, .. } if id == workspace_id => Some(Location::TopLevel(index)),
                Item::Group(group) => group
                    .members
                    .iter()
                    .position(|id| id == workspace_id)
                    .map(|member| Location::InGroup(index, member)),
                Item::Workspace { .. } => None,
            })
    }

    /// Remove a workspace and return whether it was a favorite.
    fn take_workspace(&mut self, workspace_id: &str) -> Option<bool> {
        match self.locate(workspace_id)? {
            Location::TopLevel(index) => match self.items.remove(index) {
                Item::Workspace { favorite, .. } => Some(favorite),
                Item::Group(_) => unreachable!("located a workspace"),
            },
            Location::InGroup(index, member) => {
                if let Item::Group(group) = &mut self.items[index] {
                    group.members.remove(member);
                }
                Some(false)
            }
        }
    }

    fn group_index(&self, id: &str) -> Option<usize> {
        self.items
            .iter()
            .position(|item| matches!(item, Item::Group(group) if group.id == id))
    }

    fn group_mut(&mut self, id: &str) -> Option<&mut WorkspaceGroup> {
        self.items.iter_mut().find_map(|item| match item {
            Item::Group(group) if group.id == id => Some(group),
            _ => None,
        })
    }

    fn favorites_len(&self) -> usize {
        self.items
            .iter()
            .take_while(|item| matches!(item, Item::Workspace { favorite: true, .. }))
            .count()
    }

    /// Keep favorites ahead of everything else, in their current order.
    fn settle(&mut self) {
        let (mut favorites, rest): (Vec<Item>, Vec<Item>) = std::mem::take(&mut self.items)
            .into_iter()
            .partition(|item| matches!(item, Item::Workspace { favorite: true, .. }));
        favorites.extend(rest);
        self.items = favorites;
    }

    /// The first palette colour no group uses yet, like Chrome.
    pub fn next_group_color(&self) -> WorkspaceColor {
        let used: Vec<WorkspaceColor> = self.groups().map(|group| group.color).collect();
        WorkspaceColor::ALL
            .into_iter()
            .find(|color| !used.contains(color))
            .unwrap_or(WorkspaceColor::ALL[used.len() % WorkspaceColor::ALL.len()])
    }

    fn next_group_name(&self) -> String {
        (1..)
            .map(|n| format!("Group {n}"))
            .find(|name| self.groups().all(|group| &group.name != name))
            .expect("unbounded range")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(ids: &[&str]) -> SidebarLayout {
        let mut layout = SidebarLayout::default();
        for id in ids {
            layout.push_workspace(*id, false);
        }
        layout
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    /// Rows as text: `[name]` for a header, `name/id` for a member.
    fn render(layout: &SidebarLayout) -> Vec<String> {
        layout
            .rows()
            .into_iter()
            .map(|row| match row {
                SidebarRow::GroupHeader(group) => format!("[{}]", group.name),
                SidebarRow::Workspace {
                    id,
                    group: Some(group),
                } => format!("{}/{id}", group.name),
                SidebarRow::Workspace { id, group: None } => {
                    if layout.is_favorite(id) {
                        format!("*{id}")
                    } else {
                        id.to_string()
                    }
                }
            })
            .collect()
    }

    #[test]
    fn create_group_takes_members_to_first_members_place() {
        let mut layout = layout(&["a", "b", "c", "d"]);
        let group = layout
            .create_group(Some("G"), None, &ids(&["c", "a"]))
            .unwrap();
        assert_eq!(render(&layout), ["b", "[G]", "G/c", "G/a", "d"]);
        assert_eq!(layout.workspace_order(), ids(&["b", "c", "a", "d"]));
        assert_eq!(layout.group_of("a").unwrap().id, group);
        assert!(layout.group_of("b").is_none());
    }

    #[test]
    fn create_group_defaults_name_and_unused_color() {
        let mut layout = layout(&["a"]);
        let first = layout.create_group(None, None, &[]).unwrap();
        let second = layout.create_group(None, None, &[]).unwrap();
        let first = layout.group(&first).unwrap();
        let second = layout.group(&second).unwrap();
        assert_eq!(first.name, "Group 1");
        assert_eq!(second.name, "Group 2");
        assert_eq!(first.color, WorkspaceColor::ALL[0]);
        assert_eq!(second.color, WorkspaceColor::ALL[1]);
        assert_eq!(render(&layout), ["a", "[Group 1]", "[Group 2]"]);
    }

    #[test]
    fn create_group_rejects_unknown_members_and_blank_names() {
        let mut layout = layout(&["a"]);
        assert_eq!(
            layout.create_group(None, None, &ids(&["nope"])),
            Err(GroupError::UnknownWorkspace)
        );
        assert_eq!(
            layout.create_group(Some("  "), None, &[]),
            Err(GroupError::EmptyName)
        );
        assert_eq!(render(&layout), ["a"]);
    }

    #[test]
    fn group_with_members_cannot_be_deleted() {
        let mut layout = layout(&["a", "b"]);
        let group = layout
            .create_group(Some("G"), None, &ids(&["a", "b"]))
            .unwrap();
        assert_eq!(layout.delete_group(&group), Err(GroupError::NotEmpty(2)));
        layout.set_workspace_group("a", None).unwrap();
        layout.remove_workspace("b");
        assert_eq!(render(&layout), ["[G]", "a"]);
        assert_eq!(layout.delete_group(&group), Ok(()));
        assert_eq!(render(&layout), ["a"]);
        assert_eq!(layout.delete_group(&group), Err(GroupError::UnknownGroup));
    }

    #[test]
    fn not_empty_error_names_the_count() {
        assert_eq!(
            GroupError::NotEmpty(1).to_string(),
            "group still has 1 workspace; move or close it first"
        );
        assert_eq!(
            GroupError::NotEmpty(3).to_string(),
            "group still has 3 workspaces; move or close them first"
        );
    }

    #[test]
    fn ungrouping_lands_right_after_the_group() {
        let mut layout = layout(&["a", "b", "c"]);
        layout
            .create_group(Some("G"), None, &ids(&["a", "b"]))
            .unwrap();
        layout.set_workspace_group("a", None).unwrap();
        assert_eq!(render(&layout), ["[G]", "G/b", "a", "c"]);
        // Already ungrouped: nothing moves.
        layout.set_workspace_group("c", None).unwrap();
        assert_eq!(render(&layout), ["[G]", "G/b", "a", "c"]);
    }

    #[test]
    fn set_workspace_group_appends_and_checks_ids() {
        let mut layout = layout(&["a", "b"]);
        let group = layout.create_group(Some("G"), None, &ids(&["b"])).unwrap();
        layout.set_workspace_group("a", Some(&group)).unwrap();
        assert_eq!(render(&layout), ["[G]", "G/b", "G/a"]);
        assert_eq!(
            layout.set_workspace_group("a", Some("missing")),
            Err(GroupError::UnknownGroup)
        );
        assert_eq!(
            layout.set_workspace_group("missing", None),
            Err(GroupError::UnknownWorkspace)
        );
    }

    #[test]
    fn favorites_are_never_grouped_and_stay_on_top() {
        let mut layout = layout(&["a", "b", "c"]);
        layout.set_favorite("c", true);
        assert_eq!(render(&layout), ["*c", "a", "b"]);

        // Grouping a favorite unpins it; the group takes its place.
        let group = layout.create_group(Some("G"), None, &ids(&["c"])).unwrap();
        assert_eq!(render(&layout), ["[G]", "G/c", "a", "b"]);
        assert!(!layout.is_favorite("c"));

        // Pinning a member takes it out of the group.
        layout.set_favorite("c", true);
        assert_eq!(render(&layout), ["*c", "[G]", "a", "b"]);
        layout.set_workspace_group("c", Some(&group)).unwrap();
        assert_eq!(render(&layout), ["[G]", "G/c", "a", "b"]);
    }

    #[test]
    fn unpinning_lands_after_the_remaining_favorites() {
        let mut layout = layout(&["a", "b", "c"]);
        layout.set_favorite("a", true);
        layout.set_favorite("b", true);
        assert_eq!(render(&layout), ["*a", "*b", "c"]);
        layout.set_favorite("a", false);
        assert_eq!(render(&layout), ["*b", "a", "c"]);
    }

    #[test]
    fn new_favorite_workspace_is_pushed_after_favorites() {
        let mut layout = layout(&["a"]);
        layout.push_workspace("f", true);
        assert_eq!(render(&layout), ["*f", "a"]);
    }

    #[test]
    fn dropping_next_to_members_joins_their_group() {
        let mut layout = layout(&["a", "b", "c"]);
        let group = layout
            .create_group(Some("G"), None, &ids(&["a", "b"]))
            .unwrap();
        assert!(layout.move_workspace("c", &RowRef::Workspace("a".into()), Placement::After));
        assert_eq!(render(&layout), ["[G]", "G/a", "G/c", "G/b"]);
        assert!(layout.move_workspace("b", &RowRef::Group(group.clone()), Placement::Before));
        assert_eq!(render(&layout), ["b", "[G]", "G/a", "G/c"]);
        assert!(layout.move_workspace("b", &RowRef::Group(group), Placement::After));
        assert_eq!(render(&layout), ["[G]", "G/b", "G/a", "G/c"]);
    }

    #[test]
    fn dropping_next_to_ungrouped_rows_leaves_the_group() {
        let mut layout = layout(&["a", "b", "c"]);
        layout
            .create_group(Some("G"), None, &ids(&["a", "b"]))
            .unwrap();
        assert!(layout.move_workspace("a", &RowRef::Workspace("c".into()), Placement::After));
        assert_eq!(render(&layout), ["[G]", "G/b", "c", "a"]);
    }

    #[test]
    fn moving_onto_itself_or_unknown_rows_is_refused() {
        let mut layout = layout(&["a", "b"]);
        assert!(!layout.move_workspace("a", &RowRef::Workspace("a".into()), Placement::After));
        assert!(!layout.move_workspace("a", &RowRef::Workspace("x".into()), Placement::After));
        assert!(!layout.move_workspace("x", &RowRef::Workspace("a".into()), Placement::After));
        assert!(!layout.move_workspace("a", &RowRef::Group("x".into()), Placement::After));
        assert_eq!(render(&layout), ["a", "b"]);
    }

    #[test]
    fn ungrouped_moves_keep_the_favorite_pinning() {
        let mut layout = layout(&["a", "b", "c"]);
        layout.set_favorite("a", true);
        // A favorite cannot drop below the rest, nor a regular row above favorites.
        assert!(layout.move_workspace("a", &RowRef::Workspace("c".into()), Placement::After));
        assert_eq!(render(&layout), ["*a", "b", "c"]);
        assert!(layout.move_workspace("c", &RowRef::Workspace("a".into()), Placement::Before));
        assert_eq!(render(&layout), ["*a", "c", "b"]);
    }

    #[test]
    fn dropping_a_favorite_into_a_group_unpins_it() {
        let mut layout = layout(&["a", "b"]);
        layout.create_group(Some("G"), None, &ids(&["b"])).unwrap();
        layout.set_favorite("a", true);
        assert!(layout.move_workspace("a", &RowRef::Workspace("b".into()), Placement::Before));
        assert_eq!(render(&layout), ["[G]", "G/a", "G/b"]);
        assert!(!layout.is_favorite("a"));
    }

    #[test]
    fn groups_move_as_a_block_and_never_nest() {
        let mut layout = layout(&["a", "b", "c", "d"]);
        let g = layout.create_group(Some("G"), None, &ids(&["a"])).unwrap();
        layout
            .create_group(Some("H"), None, &ids(&["c", "d"]))
            .unwrap();
        assert_eq!(render(&layout), ["[G]", "G/a", "b", "[H]", "H/c", "H/d"]);
        // Dropped on a member of H, G goes next to H rather than into it.
        assert!(layout.move_group(&g, &RowRef::Workspace("d".into()), Placement::After));
        assert_eq!(render(&layout), ["b", "[H]", "H/c", "H/d", "[G]", "G/a"]);
        assert!(layout.move_group(&g, &RowRef::Workspace("b".into()), Placement::Before));
        assert_eq!(render(&layout), ["[G]", "G/a", "b", "[H]", "H/c", "H/d"]);
        assert!(!layout.move_group(&g, &RowRef::Workspace("a".into()), Placement::After));
    }

    #[test]
    fn groups_stay_below_favorites() {
        let mut layout = layout(&["a", "b"]);
        layout.set_favorite("a", true);
        let g = layout.create_group(Some("G"), None, &ids(&["b"])).unwrap();
        assert!(layout.move_group(&g, &RowRef::Workspace("a".into()), Placement::Before));
        assert_eq!(render(&layout), ["*a", "[G]", "G/b"]);
    }

    #[test]
    fn collapsed_groups_hide_members_but_the_active_one() {
        let mut layout = layout(&["a", "b", "c"]);
        let g = layout
            .create_group(Some("G"), None, &ids(&["a", "b"]))
            .unwrap();
        layout.set_group_collapsed(&g, true).unwrap();
        fn visible(layout: &SidebarLayout, active: Option<&str>) -> Vec<String> {
            layout
                .rows()
                .iter()
                .zip(render(layout))
                .filter(|(row, _)| row.visible(active))
                .map(|(_, text)| text)
                .collect()
        }
        assert_eq!(visible(&layout, Some("c")), ["[G]", "c"]);
        assert_eq!(visible(&layout, Some("b")), ["[G]", "G/b", "c"]);
        layout.set_group_collapsed(&g, false).unwrap();
        assert_eq!(visible(&layout, Some("c")), ["[G]", "G/a", "G/b", "c"]);
    }

    #[test]
    fn rename_recolor_and_unknown_groups() {
        let mut layout = layout(&[]);
        let g = layout.create_group(Some("G"), None, &[]).unwrap();
        layout.rename_group(&g, "  Backend ").unwrap();
        layout.set_group_color(&g, WorkspaceColor::Teal).unwrap();
        let group = layout.group(&g).unwrap();
        assert_eq!(group.name, "Backend");
        assert_eq!(group.color, WorkspaceColor::Teal);
        assert_eq!(layout.rename_group(&g, " "), Err(GroupError::EmptyName));
        assert_eq!(layout.rename_group("x", "n"), Err(GroupError::UnknownGroup));
        assert_eq!(
            layout.set_group_color("x", WorkspaceColor::Red),
            Err(GroupError::UnknownGroup)
        );
        assert_eq!(
            layout.set_group_collapsed("x", true),
            Err(GroupError::UnknownGroup)
        );
    }

    #[test]
    fn saved_layout_round_trips() {
        let mut layout = layout(&["a", "b", "c", "d"]);
        layout.set_favorite("d", true);
        let g = layout
            .create_group(Some("G"), Some(WorkspaceColor::Blue), &ids(&["b", "c"]))
            .unwrap();
        layout.set_group_collapsed(&g, true).unwrap();
        layout.create_group(Some("Empty"), None, &[]).unwrap();
        layout.move_group(&g, &RowRef::Workspace("a".into()), Placement::Before);
        assert_eq!(render(&layout), ["*d", "[G]", "G/b", "G/c", "a", "[Empty]"]);

        let groups = layout.saved_groups();
        let saved: Vec<(String, bool, Option<String>)> = layout
            .workspace_order()
            .into_iter()
            .map(|id| {
                let group = layout.group_of(&id).map(|g| g.id.clone());
                (id.clone(), layout.is_favorite(&id), group)
            })
            .collect();
        let restored = SidebarLayout::from_saved(
            saved
                .iter()
                .map(|(id, fav, group)| (id.clone(), *fav, group.as_deref())),
            &groups,
        );
        assert_eq!(restored, layout);
    }

    fn saved_group(id: &str, position: usize) -> WorkspaceGroupState {
        WorkspaceGroupState {
            id: id.to_string(),
            name: id.to_uppercase(),
            color: None,
            collapsed: false,
            position,
        }
    }

    #[test]
    fn restore_repairs_inconsistent_sessions() {
        let groups = [
            saved_group("g", 0),
            saved_group("g", 5),
            saved_group("e", 1),
        ];
        let restored = SidebarLayout::from_saved(
            [
                ("a".to_string(), false, Some("g")),
                ("b".to_string(), false, None),
                // Not contiguous with `a`: gathered into the group.
                ("c".to_string(), false, Some("g")),
                // Unknown group: kept ungrouped.
                ("d".to_string(), false, Some("missing")),
                // A favorite is never grouped.
                ("f".to_string(), true, Some("g")),
            ],
            &groups,
        );
        assert_eq!(
            render(&restored),
            ["*f", "[G]", "G/a", "G/c", "[E]", "b", "d"]
        );
        // Missing colours are filled in from the unused palette.
        let colors: Vec<WorkspaceColor> = restored.groups().map(|g| g.color).collect();
        assert_eq!(colors, [WorkspaceColor::ALL[0], WorkspaceColor::ALL[1]]);
    }

    #[test]
    fn next_color_cycles_once_the_palette_is_used() {
        let mut layout = layout(&[]);
        for _ in 0..WorkspaceColor::ALL.len() {
            layout.create_group(None, None, &[]).unwrap();
        }
        assert_eq!(layout.next_group_color(), WorkspaceColor::ALL[0]);
    }
}
