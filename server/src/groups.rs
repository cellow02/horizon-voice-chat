//! Groups: players who hear each other wherever they are. The game-independent part, shared by
//! both server mods (`tools/sync-protocol.sh`); each mod sends the [`Effect`]s on its own API.

use crate::protocol::{self, ClientGroup, GroupType, PlayerState};
use std::collections::HashMap;

struct Group {
    info: ClientGroup,
    password: Option<String>,
}

/// What a change means for the clients.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Tell `player` their group (`voicechat:joined_group`).
    Joined {
        player: u128,
        group: Option<u128>,
        wrong_password: bool,
    },
    /// Tell everyone about a new group (`voicechat:add_group`).
    Added(ClientGroup),
    /// Tell everyone a group is gone (`voicechat:remove_group`).
    Removed(u128),
    /// `player`'s state changed (their group): send it to everyone.
    State(u128),
}

#[derive(Default)]
pub struct Groups {
    groups: HashMap<u128, Group>,
}

impl Groups {
    /// Every group, for a client that just joined voice chat.
    pub fn all(&self) -> impl Iterator<Item = &ClientGroup> {
        self.groups.values().map(|group| &group.info)
    }

    pub fn clear(&mut self) {
        self.groups.clear();
    }

    /// Who is in which group, for the voice server.
    pub fn membership(&self, states: &HashMap<u128, PlayerState>) -> HashMap<u128, (u128, GroupType)> {
        states
            .values()
            .filter_map(|state| {
                let group = self.groups.get(&state.group?)?;
                Some((state.uuid, (group.info.id, group.info.kind)))
            })
            .collect()
    }

    /// `voicechat:set_group`.
    pub fn join(
        &mut self,
        states: &mut HashMap<u128, PlayerState>,
        player: u128,
        group: u128,
        password: Option<&str>,
    ) -> Vec<Effect> {
        let Some(target) = self.groups.get(&group) else {
            return Vec::new();
        };
        let Some(state) = states.get_mut(&player) else {
            return Vec::new();
        };
        if state.group == Some(group) {
            return Vec::new();
        }
        if target.password.is_some() && target.password.as_deref() != password {
            return vec![Effect::Joined {
                player,
                group: None,
                wrong_password: true,
            }];
        }
        state.group = Some(group);
        let mut effects = vec![
            Effect::State(player),
            Effect::Joined {
                player,
                group: Some(group),
                wrong_password: false,
            },
        ];
        effects.extend(self.clean_up(states));
        effects
    }

    /// `voicechat:create_group`: makes the group and puts its creator in it.
    pub fn create(
        &mut self,
        states: &mut HashMap<u128, PlayerState>,
        player: u128,
        name: &str,
        password: Option<&str>,
        kind: GroupType,
    ) -> Vec<Effect> {
        if !protocol::valid_group_name(name)
            || password.is_some_and(|p| !protocol::valid_group_password(p))
            || !states.contains_key(&player)
        {
            return Vec::new();
        }
        let id = protocol::crypto::random_uuid();
        let info = ClientGroup {
            id,
            name: name.to_owned(),
            has_password: password.is_some(),
            persistent: false,
            hidden: false,
            kind,
        };
        self.groups.insert(
            id,
            Group {
                info: info.clone(),
                password: password.map(str::to_owned),
            },
        );
        let mut effects = vec![Effect::Added(info)];
        effects.extend(self.join(states, player, id, password));
        effects
    }

    /// `voicechat:leave_group`.
    pub fn leave(&mut self, states: &mut HashMap<u128, PlayerState>, player: u128) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let Some(state) = states.get_mut(&player) {
            if state.group.take().is_some() {
                effects.push(Effect::State(player));
            }
            effects.push(Effect::Joined {
                player,
                group: None,
                wrong_password: false,
            });
        }
        effects.extend(self.clean_up(states));
        effects
    }

    /// Removes groups nobody is in any more, unless they are persistent.
    pub fn clean_up(&mut self, states: &HashMap<u128, PlayerState>) -> Vec<Effect> {
        let empty: Vec<u128> = self
            .groups
            .values()
            .filter(|group| !group.info.persistent)
            .map(|group| group.info.id)
            .filter(|id| !states.values().any(|state| state.group == Some(*id)))
            .collect();
        empty
            .into_iter()
            .map(|id| {
                self.groups.remove(&id);
                Effect::Removed(id)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn players() -> HashMap<u128, PlayerState> {
        HashMap::from([
            (1, PlayerState::disconnected(1, "Alex")),
            (2, PlayerState::disconnected(2, "Steve")),
        ])
    }

    #[test]
    fn a_group_lives_while_someone_is_in_it_and_keeps_its_password() {
        let mut states = players();
        let mut groups = Groups::default();
        let effects = groups.create(&mut states, 1, "Höhle", Some("pw"), GroupType::Open);
        let Some(Effect::Added(info)) = effects.first().cloned() else {
            panic!("no group added: {effects:?}");
        };
        assert!(info.has_password && info.kind == GroupType::Open);
        assert_eq!(states[&1].group, Some(info.id));

        assert_eq!(
            groups.join(&mut states, 2, info.id, Some("wrong")),
            [Effect::Joined { player: 2, group: None, wrong_password: true }]
        );
        groups.join(&mut states, 2, info.id, Some("pw"));
        assert_eq!(states[&2].group, Some(info.id));
        assert_eq!(groups.membership(&states).len(), 2);

        groups.leave(&mut states, 1);
        assert_eq!(groups.all().count(), 1);
        let effects = groups.leave(&mut states, 2);
        assert!(effects.contains(&Effect::Removed(info.id)));
        assert_eq!(groups.all().count(), 0);
        assert!(groups.membership(&states).is_empty());
    }

    #[test]
    fn bad_names_make_no_group() {
        let mut states = players();
        let mut groups = Groups::default();
        assert!(groups.create(&mut states, 1, " ", None, GroupType::Normal).is_empty());
        assert!(groups.create(&mut states, 9, "Fine", None, GroupType::Normal).is_empty());
        assert_eq!(groups.all().count(), 0);
    }
}
