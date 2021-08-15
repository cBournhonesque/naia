use std::{
    borrow::Borrow,
    clone::Clone,
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
};

use slotmap::SparseSecondaryMap;

use byteorder::{BigEndian, WriteBytesExt};

use naia_shared::{
    DiffMask, EntityKey, KeyGenerator, LocalEntityKey, LocalReplicateKey, Manifest, NaiaKey,
    ProtocolType, Ref, Replicate, ReplicateNotifiable, MTU_SIZE,
};

use crate::packet_writer::PacketWriter;

use super::{
    entity_record::EntityRecord,
    keys::{replicate_key::ReplicateKey, ComponentKey, ObjectKey},
    locality_status::LocalityStatus,
    mut_handler::MutHandler,
    replicate_action::ReplicateAction,
    replicate_record::ReplicateRecord,
};

/// Manages Objects/Entities for a given Client connection and keeps them in
/// sync on the Client
#[derive(Debug)]
pub struct ReplicateManager<T: ProtocolType> {
    address: SocketAddr,
    // replicates
    replicate_key_generator: KeyGenerator<LocalReplicateKey>,
    local_replicate_store: SparseSecondaryMap<ReplicateKey, Ref<dyn Replicate<T>>>,
    local_to_global_replicate_key_map: HashMap<LocalReplicateKey, ReplicateKey>,
    replicate_records: SparseSecondaryMap<ReplicateKey, ReplicateRecord>,
    delayed_replicate_deletions: HashSet<ReplicateKey>,
    // objects
    pawn_object_store: HashSet<ObjectKey>,
    // entities
    entity_key_generator: KeyGenerator<LocalEntityKey>,
    local_entity_store: HashMap<EntityKey, EntityRecord>,
    local_to_global_entity_key_map: HashMap<LocalEntityKey, EntityKey>,
    pawn_entity_store: HashSet<EntityKey>,
    delayed_entity_deletions: HashSet<EntityKey>,
    // messages / updates / ect
    queued_messages: VecDeque<ReplicateAction<T>>,
    sent_messages: HashMap<u16, Vec<ReplicateAction<T>>>,
    sent_updates: HashMap<u16, HashMap<ReplicateKey, Ref<DiffMask>>>,
    last_update_packet_index: u16,
    last_last_update_packet_index: u16,
    mut_handler: Ref<MutHandler>,
    last_popped_diff_mask: Option<DiffMask>,
    last_popped_diff_mask_list: Option<Vec<(ReplicateKey, DiffMask)>>,
}

impl<T: ProtocolType> ReplicateManager<T> {
    /// Create a new ReplicateManager, given the client's address and a
    /// reference to a MutHandler associated with the Client
    pub fn new(address: SocketAddr, mut_handler: &Ref<MutHandler>) -> Self {
        ReplicateManager {
            address,
            // replicates
            replicate_key_generator: KeyGenerator::new(),
            local_replicate_store: SparseSecondaryMap::new(),
            local_to_global_replicate_key_map: HashMap::new(),
            replicate_records: SparseSecondaryMap::new(),
            pawn_object_store: HashSet::new(),
            delayed_replicate_deletions: HashSet::new(),
            // entities
            entity_key_generator: KeyGenerator::new(),
            local_to_global_entity_key_map: HashMap::new(),
            local_entity_store: HashMap::new(),
            pawn_entity_store: HashSet::new(),
            delayed_entity_deletions: HashSet::new(),
            // messages / updates / ect
            queued_messages: VecDeque::new(),
            sent_messages: HashMap::new(),
            sent_updates: HashMap::<u16, HashMap<ObjectKey, Ref<DiffMask>>>::new(),
            last_update_packet_index: 0,
            last_last_update_packet_index: 0,
            mut_handler: mut_handler.clone(),
            last_popped_diff_mask: None,
            last_popped_diff_mask_list: None,
        }
    }

    pub fn has_outgoing_actions(&self) -> bool {
        return self.queued_messages.len() != 0;
    }

    pub fn pop_outgoing_action(&mut self, packet_index: u16) -> Option<ReplicateAction<T>> {
        let queued_message_opt = self.queued_messages.pop_front();
        if queued_message_opt.is_none() {
            return None;
        }
        let mut message = queued_message_opt.unwrap();

        let replacement_message: Option<ReplicateAction<T>> = {
            match &message {
                ReplicateAction::CreateEntity(global_entity_key, local_entity_key, _) => {
                    let mut component_list = Vec::new();

                    let entity_record = self.local_entity_store.get(global_entity_key)
                        .expect("trying to pop an replicate message for an entity which has not been initialized correctly");

                    let components: &HashSet<ComponentKey> = &entity_record.components_ref.borrow();
                    for global_component_key in components {
                        let component_ref = self.local_replicate_store.get(*global_component_key)
                            .expect("trying to initiate a component which has not been initialized correctly");
                        let component_record = self.replicate_records.get(*global_component_key)
                            .expect("trying to initiate a component which has not been initialized correctly");
                        component_list.push((
                            *global_component_key,
                            component_record.local_key,
                            component_ref.clone(),
                        ));
                    }

                    Some(ReplicateAction::CreateEntity(
                        *global_entity_key,
                        *local_entity_key,
                        Some(component_list),
                    ))
                }
                _ => None,
            }
        };

        if let Some(new_message) = replacement_message {
            message = new_message;
        }

        if !self.sent_messages.contains_key(&packet_index) {
            self.sent_messages.insert(packet_index, Vec::new());
        }

        if let Some(sent_messages_list) = self.sent_messages.get_mut(&packet_index) {
            sent_messages_list.push(message.clone());
        }

        //clear replicate mask of replicate if need be
        match &message {
            ReplicateAction::CreateObject(global_key, _, _) => {
                self.pop_create_replicate_diff_mask(global_key);
            }
            ReplicateAction::AddComponent(_, global_key, _, _) => {
                self.pop_create_replicate_diff_mask(global_key);
            }
            ReplicateAction::CreateEntity(_, _, components_list_opt) => {
                if let Some(components_list) = components_list_opt {
                    let mut diff_mask_list: Vec<(ComponentKey, DiffMask)> = Vec::new();
                    for (global_component_key, _, _) in components_list {
                        if let Some(record) = self.replicate_records.get(*global_component_key) {
                            diff_mask_list.push((
                                *global_component_key,
                                record.get_diff_mask().borrow().clone(),
                            ));
                        }
                        self.mut_handler
                            .borrow_mut()
                            .clear_replicate(&self.address, global_component_key);
                    }
                    self.last_popped_diff_mask_list = Some(diff_mask_list);
                }
            }
            ReplicateAction::UpdateReplicate(global_key, local_key, diff_mask, replicate) => {
                return Some(self.pop_update_replicate_diff_mask(
                    false,
                    packet_index,
                    global_key,
                    local_key,
                    diff_mask,
                    replicate,
                ));
            }
            ReplicateAction::UpdatePawn(global_key, local_key, diff_mask, replicate) => {
                return Some(self.pop_update_replicate_diff_mask(
                    true,
                    packet_index,
                    global_key,
                    local_key,
                    diff_mask,
                    replicate,
                ));
            }
            _ => {}
        }

        return Some(message);
    }

    pub fn unpop_outgoing_action(&mut self, packet_index: u16, message: &ReplicateAction<T>) {
        info!("unpopping");
        if let Some(sent_messages_list) = self.sent_messages.get_mut(&packet_index) {
            sent_messages_list.pop();
            if sent_messages_list.len() == 0 {
                self.sent_messages.remove(&packet_index);
            }
        }

        match &message {
            ReplicateAction::CreateObject(global_key, _, _) => {
                self.unpop_create_replicate_diff_mask(global_key);
            }
            ReplicateAction::AddComponent(_, global_key, _, _) => {
                self.unpop_create_replicate_diff_mask(global_key);
            }
            ReplicateAction::CreateEntity(_, _, _) => {
                if let Some(last_popped_diff_mask_list) = &self.last_popped_diff_mask_list {
                    for (global_component_key, last_popped_diff_mask) in last_popped_diff_mask_list
                    {
                        self.mut_handler.borrow_mut().set_replicate(
                            &self.address,
                            global_component_key,
                            &last_popped_diff_mask,
                        );
                    }
                }
            }
            ReplicateAction::UpdateReplicate(global_key, local_key, _, replicate) => {
                let cloned_message = self.unpop_update_replicate_diff_mask(
                    false,
                    packet_index,
                    global_key,
                    local_key,
                    replicate,
                );
                self.queued_messages.push_front(cloned_message);
                return;
            }
            ReplicateAction::UpdatePawn(global_key, local_key, _, replicate) => {
                let cloned_message = self.unpop_update_replicate_diff_mask(
                    true,
                    packet_index,
                    global_key,
                    local_key,
                    replicate,
                );
                self.queued_messages.push_front(cloned_message);
                return;
            }
            _ => {}
        }

        self.queued_messages.push_front(message.clone());
    }

    // Replicates

    pub fn add_object(&mut self, key: &ObjectKey, replicate: &Ref<dyn Replicate<T>>) {
        let local_key = self.replicate_init(key, replicate, LocalityStatus::Creating);

        self.queued_messages
            .push_back(ReplicateAction::CreateObject(
                *key,
                local_key,
                replicate.clone(),
            ));
    }

    pub fn remove_object(&mut self, key: &ObjectKey) {
        if self.has_pawn(key) {
            self.remove_pawn(key);
        }

        if let Some(replicate_record) = self.replicate_records.get_mut(*key) {
            match replicate_record.status {
                LocalityStatus::Creating => {
                    // queue deletion message to be sent after creation
                    self.delayed_replicate_deletions.insert(*key);
                }
                LocalityStatus::Created => {
                    // send deletion message
                    replicate_delete(&mut self.queued_messages, replicate_record, key);
                }
                LocalityStatus::Deleting => {
                    // deletion in progress, do nothing
                }
            }
        } else {
            panic!("attempting to remove an replicate from a connection within which it does not exist");
        }
    }

    pub fn has_object(&self, key: &ObjectKey) -> bool {
        return self.local_replicate_store.contains_key(*key);
    }

    // Pawns

    pub fn add_pawn(&mut self, key: &ObjectKey) {
        if self.local_replicate_store.contains_key(*key) {
            if !self.pawn_object_store.contains(key) {
                self.pawn_object_store.insert(*key);
                if let Some(replicate_record) = self.replicate_records.get_mut(*key) {
                    self.queued_messages.push_back(ReplicateAction::AssignPawn(
                        *key,
                        replicate_record.local_key,
                    ));
                }
            }
        } else {
            panic!("user connection does not have local replicate to make into a pawn!");
        }
    }

    pub fn remove_pawn(&mut self, key: &ObjectKey) {
        if self.pawn_object_store.remove(key) {
            if let Some(replicate_record) = self.replicate_records.get_mut(*key) {
                self.queued_messages
                    .push_back(ReplicateAction::UnassignPawn(
                        *key,
                        replicate_record.local_key,
                    ));
            }
        } else {
            panic!("attempt to unassign a pawn replicate from a connection to which it is not assigned as a pawn in the first place")
        }
    }

    pub fn has_pawn(&self, key: &ObjectKey) -> bool {
        return self.pawn_object_store.contains(key);
    }

    // Entities

    pub fn add_entity(
        &mut self,
        global_key: &EntityKey,
        components_ref: &Ref<HashSet<ComponentKey>>,
        component_list: &Vec<(ComponentKey, Ref<dyn Replicate<T>>)>,
    ) {
        if !self.local_entity_store.contains_key(global_key) {
            // first, add components
            for (component_key, component_ref) in component_list {
                self.replicate_init(component_key, component_ref, LocalityStatus::Creating);
            }

            // then, add entity
            let local_key: LocalEntityKey = self.entity_key_generator.generate();
            self.local_to_global_entity_key_map
                .insert(local_key, *global_key);
            let entity_record = EntityRecord::new(local_key, components_ref);
            self.local_entity_store.insert(*global_key, entity_record);
            self.queued_messages
                .push_back(ReplicateAction::CreateEntity(*global_key, local_key, None));
        } else {
            panic!("added entity twice");
        }
    }

    pub fn remove_entity(&mut self, key: &EntityKey) {
        if self.has_pawn_entity(key) {
            self.remove_pawn_entity(key);
        }

        if let Some(entity_record) = self.local_entity_store.get_mut(key) {
            match entity_record.status {
                LocalityStatus::Creating => {
                    // queue deletion message to be sent after creation
                    self.delayed_entity_deletions.insert(*key);
                }
                LocalityStatus::Created => {
                    // send deletion message
                    entity_delete(&mut self.queued_messages, entity_record, key);

                    // Entity deletion IS Component deletion, so update those replicate records
                    // accordingly
                    let component_set: &HashSet<ComponentKey> =
                        &entity_record.components_ref.borrow();
                    for component_key in component_set {
                        self.pawn_object_store.remove(component_key);

                        if let Some(replicate_record) =
                            self.replicate_records.get_mut(*component_key)
                        {
                            replicate_record.status = LocalityStatus::Deleting;
                        }
                    }
                }
                LocalityStatus::Deleting => {
                    // deletion in progress, do nothing
                }
            }
        }
    }

    pub fn has_entity(&self, key: &EntityKey) -> bool {
        return self.local_entity_store.contains_key(key);
    }

    // Pawn Entities

    pub fn add_pawn_entity(&mut self, key: &EntityKey) {
        if self.local_entity_store.contains_key(key) {
            if !self.pawn_entity_store.contains(key) {
                self.pawn_entity_store.insert(*key);
                let local_key = self.local_entity_store.get(key).unwrap().local_key;
                self.queued_messages
                    .push_back(ReplicateAction::AssignPawnEntity(*key, local_key));
            } else {
                warn!("attempting to assign a pawn entity twice");
            }
        } else {
            warn!("attempting to assign a nonexistent entity to be a pawn");
        }
    }

    pub fn remove_pawn_entity(&mut self, key: &EntityKey) {
        if self.pawn_entity_store.contains(key) {
            self.pawn_entity_store.remove(key);
            let local_key = self
                .local_entity_store
                .get(key)
                .expect(
                    "expecting an entity record to exist if that entity is designated as a pawn",
                )
                .local_key;

            self.queued_messages
                .push_back(ReplicateAction::UnassignPawnEntity(*key, local_key));
        } else {
            panic!("attempting to unassign an entity as a pawn which is not assigned as a pawn in the first place")
        }
    }

    pub fn has_pawn_entity(&self, key: &EntityKey) -> bool {
        return self.pawn_entity_store.contains(key);
    }

    // Components

    // called when the entity already exists in this connection
    pub fn add_component(
        &mut self,
        entity_key: &EntityKey,
        component_key: &ComponentKey,
        component_ref: &Ref<dyn Replicate<T>>,
    ) {
        if !self.local_entity_store.contains_key(entity_key) {
            panic!(
                "attempting to add component to entity that does not yet exist for this connection"
            );
        }

        let local_component_key =
            self.replicate_init(component_key, component_ref, LocalityStatus::Creating);

        let entity_record = self.local_entity_store.get(entity_key).unwrap();

        match entity_record.status {
            LocalityStatus::Creating => {
                // uncreated components will be created after entity is created
            }
            LocalityStatus::Created => {
                // send add component message
                self.queued_messages
                    .push_back(ReplicateAction::AddComponent(
                        entity_record.local_key,
                        *component_key,
                        local_component_key,
                        component_ref.clone(),
                    ));
            }
            LocalityStatus::Deleting => {
                // deletion in progress, do nothing
            }
        }
    }

    // Ect..

    pub fn get_global_key_from_local(&self, local_key: LocalReplicateKey) -> Option<&ObjectKey> {
        return self.local_to_global_replicate_key_map.get(&local_key);
    }

    pub fn get_global_entity_key_from_local(
        &self,
        local_key: LocalEntityKey,
    ) -> Option<&EntityKey> {
        return self.local_to_global_entity_key_map.get(&local_key);
    }

    pub fn collect_replicate_updates(&mut self) {
        for (key, record) in self.replicate_records.iter() {
            if record.status == LocalityStatus::Created
                && !record.get_diff_mask().borrow().is_clear()
            {
                if let Some(replicate_ref) = self.local_replicate_store.get(key) {
                    if self.pawn_object_store.contains(&key) {
                        // handle as a pawn
                        self.queued_messages.push_back(ReplicateAction::UpdatePawn(
                            key,
                            record.local_key,
                            record.get_diff_mask().clone(),
                            replicate_ref.clone(),
                        ));
                    } else {
                        // handle as a replicate (object or component)
                        self.queued_messages
                            .push_back(ReplicateAction::UpdateReplicate(
                                key,
                                record.local_key,
                                record.get_diff_mask().clone(),
                                replicate_ref.clone(),
                            ));
                    }
                }
            }
        }
    }

    pub fn write_replicate_action(
        &self,
        packet_writer: &mut PacketWriter,
        manifest: &Manifest<T>,
        message: &ReplicateAction<T>,
    ) -> bool {
        let mut replicate_total_bytes = Vec::<u8>::new();

        //Write replicate message type
        replicate_total_bytes
            .write_u8(message.as_type().to_u8())
            .unwrap(); // write replicate message type

        match message {
            ReplicateAction::CreateObject(_, local_key, replicate) => {
                //write replicate payload
                let mut replicate_payload_bytes = Vec::<u8>::new();
                replicate.borrow().write(&mut replicate_payload_bytes);

                //Write replicate "header"
                let type_id = replicate.borrow().get_type_id();
                let naia_id = manifest.get_naia_id(&type_id); // get naia id
                replicate_total_bytes
                    .write_u16::<BigEndian>(naia_id)
                    .unwrap(); // write naia id
                replicate_total_bytes
                    .write_u16::<BigEndian>(local_key.to_u16())
                    .unwrap(); //write local key
                replicate_total_bytes.append(&mut replicate_payload_bytes); // write payload
            }
            ReplicateAction::DeleteReplicate(_, local_key) => {
                replicate_total_bytes
                    .write_u16::<BigEndian>(local_key.to_u16())
                    .unwrap(); //write local key
            }
            ReplicateAction::UpdateReplicate(_, local_key, diff_mask, replicate) => {
                //write replicate payload
                let mut replicate_payload_bytes = Vec::<u8>::new();
                replicate
                    .borrow()
                    .write_partial(&diff_mask.borrow(), &mut replicate_payload_bytes);

                //Write replicate "header"
                replicate_total_bytes
                    .write_u16::<BigEndian>(local_key.to_u16())
                    .unwrap(); //write local key
                diff_mask.borrow_mut().write(&mut replicate_total_bytes); // write replicate mask
                replicate_total_bytes.append(&mut replicate_payload_bytes); // write payload
            }
            ReplicateAction::AssignPawn(_, local_key) => {
                replicate_total_bytes
                    .write_u16::<BigEndian>(local_key.to_u16())
                    .unwrap(); //write local key
            }
            ReplicateAction::UnassignPawn(_, local_key) => {
                replicate_total_bytes
                    .write_u16::<BigEndian>(local_key.to_u16())
                    .unwrap(); //write local key
            }
            ReplicateAction::UpdatePawn(_, local_key, _, replicate) => {
                //write replicate payload
                let mut replicate_payload_bytes = Vec::<u8>::new();
                replicate.borrow().write(&mut replicate_payload_bytes);

                //Write replicate "header"
                replicate_total_bytes
                    .write_u16::<BigEndian>(local_key.to_u16())
                    .unwrap(); //write local key
                replicate_total_bytes.append(&mut replicate_payload_bytes); // write payload
            }
            ReplicateAction::CreateEntity(_, local_entity_key, component_list_opt) => {
                replicate_total_bytes
                    .write_u16::<BigEndian>(local_entity_key.to_u16())
                    .unwrap(); //write local entity key

                // get list of components
                if let Some(component_list) = component_list_opt {
                    let components_num = component_list.len();
                    if components_num > 255 {
                        panic!("no entity should have so many components... fix this");
                    }
                    replicate_total_bytes
                        .write_u8(components_num as u8)
                        .unwrap(); //write number of components

                    for (_, local_component_key, component_ref) in component_list {
                        //write component payload
                        let mut component_payload_bytes = Vec::<u8>::new();
                        component_ref.borrow().write(&mut component_payload_bytes);

                        //Write component "header"
                        let type_id = component_ref.borrow().get_type_id();
                        let naia_id = manifest.get_naia_id(&type_id); // get naia id
                        replicate_total_bytes
                            .write_u16::<BigEndian>(naia_id)
                            .unwrap(); // write naia id
                        replicate_total_bytes
                            .write_u16::<BigEndian>(local_component_key.to_u16())
                            .unwrap(); //write local key
                        replicate_total_bytes.append(&mut component_payload_bytes);
                        // write payload
                    }
                } else {
                    replicate_total_bytes.write_u8(0).unwrap();
                }
            }
            ReplicateAction::DeleteEntity(_, local_key) => {
                replicate_total_bytes
                    .write_u16::<BigEndian>(local_key.to_u16())
                    .unwrap(); //write local key
            }
            ReplicateAction::AssignPawnEntity(_, local_key) => {
                replicate_total_bytes
                    .write_u16::<BigEndian>(local_key.to_u16())
                    .unwrap(); //write local key
            }
            ReplicateAction::UnassignPawnEntity(_, local_key) => {
                replicate_total_bytes
                    .write_u16::<BigEndian>(local_key.to_u16())
                    .unwrap(); //write local key
            }
            ReplicateAction::AddComponent(local_entity_key, _, local_component_key, component) => {
                //write component payload
                let mut component_payload_bytes = Vec::<u8>::new();
                component.borrow().write(&mut component_payload_bytes);

                //Write component "header"
                replicate_total_bytes
                    .write_u16::<BigEndian>(local_entity_key.to_u16())
                    .unwrap(); //write local entity key
                let type_id = component.borrow().get_type_id();
                let naia_id = manifest.get_naia_id(&type_id); // get naia id
                replicate_total_bytes
                    .write_u16::<BigEndian>(naia_id)
                    .unwrap(); // write naia id
                replicate_total_bytes
                    .write_u16::<BigEndian>(local_component_key.to_u16())
                    .unwrap(); //write local component key
                replicate_total_bytes.append(&mut component_payload_bytes); // write payload
            }
        }

        let mut hypothetical_next_payload_size =
            packet_writer.bytes_number() + replicate_total_bytes.len();
        if packet_writer.replicate_action_count == 0 {
            hypothetical_next_payload_size += 2;
        }
        if hypothetical_next_payload_size < MTU_SIZE {
            if packet_writer.replicate_action_count == 255 {
                return false;
            }
            packet_writer.replicate_action_count =
                packet_writer.replicate_action_count.wrapping_add(1);
            packet_writer
                .replicate_working_bytes
                .append(&mut replicate_total_bytes);
            return true;
        } else {
            return false;
        }
    }

    // Private methods

    fn replicate_init(
        &mut self,
        key: &ObjectKey,
        replicate: &Ref<dyn Replicate<T>>,
        status: LocalityStatus,
    ) -> LocalReplicateKey {
        if !self.local_replicate_store.contains_key(*key) {
            self.local_replicate_store.insert(*key, replicate.clone());
            let local_key: LocalReplicateKey = self.replicate_key_generator.generate();
            self.local_to_global_replicate_key_map
                .insert(local_key, *key);
            let diff_mask_size = replicate.borrow().get_diff_mask_size();
            let replicate_record = ReplicateRecord::new(local_key, diff_mask_size, status);
            self.mut_handler.borrow_mut().register_mask(
                &self.address,
                &key,
                replicate_record.get_diff_mask(),
            );
            self.replicate_records.insert(*key, replicate_record);
            return local_key;
        } else {
            // Should panic, as this is not dependent on any unreliable transport freplicate
            panic!("attempted to add replicate twice..");
        }
    }

    fn replicate_cleanup(&mut self, global_object_key: &ObjectKey) {
        if let Some(replicate_record) = self.replicate_records.remove(*global_object_key) {
            // actually delete the replicate from local records
            let local_object_key = replicate_record.local_key;
            self.mut_handler
                .borrow_mut()
                .deregister_mask(&self.address, global_object_key);
            self.local_replicate_store.remove(*global_object_key);
            self.local_to_global_replicate_key_map
                .remove(&local_object_key);
            self.replicate_key_generator.recycle_key(&local_object_key);
            self.pawn_object_store.remove(&global_object_key);
        } else {
            // likely due to duplicate delivered deletion messages
            warn!(
                "attempting to clean up replicate from connection inside which it is not present"
            );
        }
    }

    fn pop_create_replicate_diff_mask(&mut self, global_key: &ObjectKey) {
        if let Some(record) = self.replicate_records.get(*global_key) {
            self.last_popped_diff_mask = Some(record.get_diff_mask().borrow().clone());
        }
        self.mut_handler
            .borrow_mut()
            .clear_replicate(&self.address, global_key);
    }

    fn unpop_create_replicate_diff_mask(&mut self, global_key: &ObjectKey) {
        if let Some(last_popped_diff_mask) = &self.last_popped_diff_mask {
            self.mut_handler.borrow_mut().set_replicate(
                &self.address,
                global_key,
                &last_popped_diff_mask,
            );
        }
    }

    fn pop_update_replicate_diff_mask(
        &mut self,
        is_pawn: bool,
        packet_index: u16,
        global_key: &ObjectKey,
        local_key: &LocalReplicateKey,
        diff_mask: &Ref<DiffMask>,
        replicate: &Ref<dyn Replicate<T>>,
    ) -> ReplicateAction<T> {
        let locked_diff_mask = self.process_replicate_update(packet_index, global_key, diff_mask);
        // return new Update message to be written
        if is_pawn {
            return ReplicateAction::UpdatePawn(
                *global_key,
                *local_key,
                locked_diff_mask,
                replicate.clone(),
            );
        } else {
            return ReplicateAction::UpdateReplicate(
                *global_key,
                *local_key,
                locked_diff_mask,
                replicate.clone(),
            );
        }
    }

    fn unpop_update_replicate_diff_mask(
        &mut self,
        is_pawn: bool,
        packet_index: u16,
        global_key: &ObjectKey,
        local_key: &LocalReplicateKey,
        replicate: &Ref<dyn Replicate<T>>,
    ) -> ReplicateAction<T> {
        let original_diff_mask = self.undo_replicate_update(&packet_index, &global_key);
        if is_pawn {
            return ReplicateAction::UpdatePawn(
                *global_key,
                *local_key,
                original_diff_mask,
                replicate.clone(),
            );
        } else {
            return ReplicateAction::UpdateReplicate(
                *global_key,
                *local_key,
                original_diff_mask,
                replicate.clone(),
            );
        }
    }

    fn process_replicate_update(
        &mut self,
        packet_index: u16,
        global_key: &ObjectKey,
        diff_mask: &Ref<DiffMask>,
    ) -> Ref<DiffMask> {
        // previously the replicate mask was the CURRENT replicate mask for the
        // replicate, we want to lock that in so we know exactly what we're
        // writing
        let locked_diff_mask = Ref::new(diff_mask.borrow().clone());

        // place replicate mask in a special transmission record - like map
        if !self.sent_updates.contains_key(&packet_index) {
            let sent_updates_map: HashMap<ObjectKey, Ref<DiffMask>> = HashMap::new();
            self.sent_updates.insert(packet_index, sent_updates_map);
            self.last_last_update_packet_index = self.last_update_packet_index;
            self.last_update_packet_index = packet_index;
        }

        if let Some(sent_updates_map) = self.sent_updates.get_mut(&packet_index) {
            sent_updates_map.insert(*global_key, locked_diff_mask.clone());
        }

        // having copied the replicate mask for this update, clear the replicate
        self.last_popped_diff_mask = Some(diff_mask.borrow().clone());
        self.mut_handler
            .borrow_mut()
            .clear_replicate(&self.address, global_key);

        locked_diff_mask
    }

    fn undo_replicate_update(
        &mut self,
        packet_index: &u16,
        global_key: &ObjectKey,
    ) -> Ref<DiffMask> {
        if let Some(sent_updates_map) = self.sent_updates.get_mut(packet_index) {
            sent_updates_map.remove(global_key);
            if sent_updates_map.len() == 0 {
                self.sent_updates.remove(&packet_index);
            }
        }

        self.last_update_packet_index = self.last_last_update_packet_index;
        if let Some(last_popped_diff_mask) = &self.last_popped_diff_mask {
            self.mut_handler.borrow_mut().set_replicate(
                &self.address,
                global_key,
                &last_popped_diff_mask,
            );
        }

        self.replicate_records
            .get(*global_key)
            .expect("uh oh, we don't have enough info to unpop the message")
            .get_diff_mask()
            .clone()
    }
}

impl<T: ProtocolType> ReplicateNotifiable for ReplicateManager<T> {
    fn notify_packet_delivered(&mut self, packet_index: u16) {
        let mut deleted_replicates: Vec<ObjectKey> = Vec::new();

        if let Some(delivered_messages_list) = self.sent_messages.remove(&packet_index) {
            for delivered_message in delivered_messages_list.into_iter() {
                match delivered_message {
                    ReplicateAction::CreateObject(global_key, _, _) => {
                        let replicate_record = self.replicate_records.get_mut(global_key)
                            .expect("created Object does not have an replicate_record ... initialization error?");

                        // do we need to delete this now?
                        if self.delayed_replicate_deletions.remove(&global_key) {
                            replicate_delete(
                                &mut self.queued_messages,
                                replicate_record,
                                &global_key,
                            );
                        } else {
                            // we do not need to delete just yet
                            replicate_record.status = LocalityStatus::Created;
                        }
                    }
                    ReplicateAction::DeleteReplicate(global_object_key, _) => {
                        deleted_replicates.push(global_object_key);
                    }
                    ReplicateAction::UpdateReplicate(_, _, _, _)
                    | ReplicateAction::UpdatePawn(_, _, _, _) => {
                        self.sent_updates.remove(&packet_index);
                    }
                    ReplicateAction::AssignPawn(_, _) => {}
                    ReplicateAction::UnassignPawn(_, _) => {}
                    ReplicateAction::CreateEntity(global_entity_key, _, component_list_opt) => {
                        let entity_record = self.local_entity_store.get_mut(&global_entity_key)
                            .expect("created entity does not have a entity_record ... initialization error?");

                        // do we need to delete this now?
                        if self.delayed_entity_deletions.remove(&global_entity_key) {
                            entity_delete(
                                &mut self.queued_messages,
                                entity_record,
                                &global_entity_key,
                            );
                        } else {
                            // set to status of created
                            entity_record.status = LocalityStatus::Created;

                            // set status of components to created
                            if let Some(mut component_list) = component_list_opt {
                                while let Some((global_component_key, _, _)) = component_list.pop()
                                {
                                    let component_record = self
                                        .replicate_records
                                        .get_mut(global_component_key)
                                        .expect("component not created correctly?");
                                    component_record.status = LocalityStatus::Created;
                                }
                            }

                            // for any components on this entity that have not yet been created
                            // initiate that now
                            let component_set: &HashSet<ComponentKey> =
                                &entity_record.components_ref.borrow();
                            for component_key in component_set {
                                let component_record = self
                                    .replicate_records
                                    .get(*component_key)
                                    .expect("component not created correctly?");
                                // check if component has been successfully created
                                // (perhaps through the previous entity_create operation)
                                if component_record.status == LocalityStatus::Creating {
                                    let component_ref = self
                                        .local_replicate_store
                                        .get(*component_key)
                                        .expect("component not created correctly?");
                                    self.queued_messages
                                        .push_back(ReplicateAction::AddComponent(
                                            entity_record.local_key,
                                            *component_key,
                                            component_record.local_key,
                                            component_ref.clone(),
                                        ));
                                }
                            }
                        }
                    }
                    ReplicateAction::DeleteEntity(global_key, local_key) => {
                        let entity_record = self
                            .local_entity_store
                            .remove(&global_key)
                            .expect("deletion of nonexistent entity!");

                        // actually delete the entity from local records
                        self.local_to_global_entity_key_map.remove(&local_key);
                        self.entity_key_generator.recycle_key(&local_key);
                        self.pawn_entity_store.remove(&global_key);

                        // delete all associated component replicates
                        let component_set: &HashSet<ComponentKey> =
                            &entity_record.components_ref.borrow();
                        for component_key in component_set {
                            deleted_replicates.push(*component_key);
                        }
                    }
                    ReplicateAction::AssignPawnEntity(_, _) => {}
                    ReplicateAction::UnassignPawnEntity(_, _) => {}
                    ReplicateAction::AddComponent(_, global_component_key, _, _) => {
                        let component_record =
                            self.replicate_records.get_mut(global_component_key).expect(
                                "added component does not have a record .. initiation problem?",
                            );
                        // do we need to delete this now?
                        if self
                            .delayed_replicate_deletions
                            .remove(&global_component_key)
                        {
                            replicate_delete(
                                &mut self.queued_messages,
                                component_record,
                                &global_component_key,
                            );
                        } else {
                            // we do not need to delete just yet
                            component_record.status = LocalityStatus::Created;
                        }
                    }
                }
            }
        }

        for deleted_object_key in deleted_replicates {
            self.replicate_cleanup(&deleted_object_key);
        }
    }

    fn notify_packet_dropped(&mut self, dropped_packet_index: u16) {
        if let Some(dropped_messages_list) = self.sent_messages.get(&dropped_packet_index) {
            for dropped_message in dropped_messages_list.into_iter() {
                match dropped_message {
                    // gauranteed delivery messages
                    ReplicateAction::CreateObject(_, _, _)
                    | ReplicateAction::DeleteReplicate(_, _)
                    | ReplicateAction::AssignPawn(_, _)
                    | ReplicateAction::UnassignPawn(_, _)
                    | ReplicateAction::CreateEntity(_, _, _)
                    | ReplicateAction::DeleteEntity(_, _)
                    | ReplicateAction::AssignPawnEntity(_, _)
                    | ReplicateAction::UnassignPawnEntity(_, _)
                    | ReplicateAction::AddComponent(_, _, _, _) => {
                        self.queued_messages.push_back(dropped_message.clone());
                    }
                    // non-gauranteed delivery messages
                    ReplicateAction::UpdateReplicate(global_key, _, _, _)
                    | ReplicateAction::UpdatePawn(global_key, _, _, _) => {
                        if let Some(diff_mask_map) = self.sent_updates.get(&dropped_packet_index) {
                            if let Some(diff_mask) = diff_mask_map.get(global_key) {
                                let mut new_diff_mask = diff_mask.borrow().clone();

                                // walk from dropped packet up to most recently sent packet
                                if dropped_packet_index != self.last_update_packet_index {
                                    let mut packet_index = dropped_packet_index.wrapping_add(1);
                                    while packet_index != self.last_update_packet_index {
                                        if let Some(diff_mask_map) =
                                            self.sent_updates.get(&packet_index)
                                        {
                                            if let Some(diff_mask) = diff_mask_map.get(global_key) {
                                                new_diff_mask.nand(diff_mask.borrow().borrow());
                                            }
                                        }

                                        packet_index = packet_index.wrapping_add(1);
                                    }
                                }

                                if let Some(record) = self.replicate_records.get_mut(*global_key) {
                                    let mut current_diff_mask = record.get_diff_mask().borrow_mut();
                                    current_diff_mask.or(new_diff_mask.borrow());
                                }
                            }
                        }
                    }
                }
            }

            self.sent_updates.remove(&dropped_packet_index);
            self.sent_messages.remove(&dropped_packet_index);
        }
    }
}

fn replicate_delete<T: ProtocolType>(
    queued_messages: &mut VecDeque<ReplicateAction<T>>,
    replicate_delete: &mut ReplicateRecord,
    object_key: &ObjectKey,
) {
    replicate_delete.status = LocalityStatus::Deleting;

    queued_messages.push_back(ReplicateAction::DeleteReplicate(
        *object_key,
        replicate_delete.local_key,
    ));
}

fn entity_delete<T: ProtocolType>(
    queued_messages: &mut VecDeque<ReplicateAction<T>>,
    entity_record: &mut EntityRecord,
    entity_key: &EntityKey,
) {
    entity_record.status = LocalityStatus::Deleting;

    queued_messages.push_back(ReplicateAction::DeleteEntity(
        *entity_key,
        entity_record.local_key,
    ));
}
