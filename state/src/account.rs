use cita_trie::codec::RLPNodeCodec;
use cita_trie::db::DB;
use cita_trie::trie::{PatriciaTrie, Trie};
use ethereum_types::{H256, U256};
use sha3::{Digest, Sha3_256};
use std::collections::HashMap;

pub const SHA3_EMPTY: H256 = H256([
    167, 255, 198, 248, 191, 30, 215, 102, 81, 193, 71, 86, 160, 97, 214, 98, 245, 128, 255, 77,
    228, 59, 73, 250, 130, 216, 10, 75, 128, 248, 67, 74,
]);
pub const SHA3_NULL_RLP: H256 = H256([
    188, 32, 113, 164, 222, 132, 111, 40, 87, 2, 68, 127, 37, 137, 221, 22, 54, 120, 224, 151, 42,
    138, 27, 13, 40, 176, 78, 213, 192, 148, 84, 127,
]);

#[derive(Debug)]
pub struct Account {
    balance: U256,
    nonce: U256,
    storage_root: H256,
    code_hash: H256,
}

impl rlp::Encodable for Account {
    fn rlp_append(&self, s: &mut rlp::RlpStream) {
        s.begin_list(4)
            .append(&self.nonce)
            .append(&self.balance)
            .append(&self.storage_root)
            .append(&self.code_hash);
    }
}

impl rlp::Decodable for Account {
    fn decode(data: &rlp::Rlp) -> Result<Self, rlp::DecoderError> {
        Ok(Account {
            nonce: data.val_at(0)?,
            balance: data.val_at(1)?,
            storage_root: data.val_at(2)?,
            code_hash: data.val_at(3)?,
        })
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum CodeState {
    Clean,
    Dirty,
}

#[derive(Debug)]
pub struct StateObject {
    balance: U256,
    nonce: U256,
    storage_root: H256,
    code_hash: H256,
    code: Vec<u8>,
    code_size: usize,
    code_state: CodeState,
    storage_changes: HashMap<H256, H256>,
}

impl From<Account> for StateObject {
    fn from(account: Account) -> Self {
        StateObject {
            balance: account.balance,
            nonce: account.nonce,
            storage_root: account.storage_root,
            code_hash: account.code_hash,
            code: vec![],
            code_size: 0,
            code_state: CodeState::Clean,
            storage_changes: HashMap::new(),
        }
    }
}

impl StateObject {
    /// Create a new account.
    /// NOTE: If contract account generated, make sure you use `init_code` on
    /// this before `commit`ing.
    pub fn new(balance: U256, nonce: U256) -> StateObject {
        StateObject {
            balance: balance,
            nonce: nonce,
            storage_root: SHA3_NULL_RLP,
            code_hash: SHA3_EMPTY,
            code: vec![],
            code_size: 0,
            code_state: CodeState::Clean,
            storage_changes: HashMap::new(),
        }
    }

    /// Create a new account from rlp bytes.
    /// Note: make sure you use `read_code` after this.
    pub fn from_rlp(data: &[u8]) -> StateObject {
        let account: Account = rlp::decode(data).unwrap();
        account.into()
    }

    pub fn rlp(&self) -> Vec<u8> {
        rlp::encode(&self.account())
    }

    pub fn account(&self) -> Account {
        Account {
            balance: self.balance,
            nonce: self.nonce,
            storage_root: self.storage_root,
            code_hash: self.code_hash,
        }
    }

    pub fn init_code(&mut self, code: Vec<u8>) {
        self.code = code.clone();
        self.code_hash = From::from(&Sha3_256::digest(&code)[..]);
        self.code_size = code.len();
        self.code_state = CodeState::Dirty;
    }

    pub fn read_code<B: DB>(&mut self, db: &mut B) -> Vec<u8> {
        if self.code_hash == SHA3_EMPTY {
            return vec![];
        }
        if !self.code.is_empty() {
            return self.code.clone();
        }
        let c = db.get(&self.code_hash).unwrap().unwrap();
        self.code = c.clone();
        self.code_size = c.len();
        self.code_state = CodeState::Clean;
        c
    }

    pub fn balance(&self) -> U256 {
        self.balance.clone()
    }

    pub fn nonce(&self) -> U256 {
        self.nonce.clone()
    }

    pub fn code(&self) -> Option<Vec<u8>> {
        if self.code.is_empty() {
            return None;
        }
        Some(self.code.clone())
    }

    pub fn code_hash(&self) -> H256 {
        self.code_hash.clone()
    }

    pub fn code_size(&self) -> usize {
        self.code_size
    }

    pub fn inc_nonce(&mut self) {
        self.nonce = self.nonce + U256::from(1u8);
    }

    pub fn add_balance(&mut self, x: U256) {
        let (a, b) = self.balance.overflowing_add(x);
        assert_eq!(b, false);
        self.balance = a;
    }

    pub fn sub_balance(&mut self, x: U256) {
        let (a, b) = self.balance.overflowing_sub(x);
        assert_eq!(b, false);
        self.balance = a;
    }

    pub fn set_storage(&mut self, key: H256, value: H256) {
        self.storage_changes.insert(key, value);
    }

    pub fn get_storage_at_backend<B: DB>(&mut self, db: &mut B, key: &H256) -> Option<H256> {
        let trie = PatriciaTrie::from(db, RLPNodeCodec::default(), &self.storage_root.0).unwrap();
        if let Ok(a) = trie.get(key) {
            if let Some(b) = a {
                return Some(From::from(&b[..]));
            }
        }
        return None;
    }

    pub fn get_storage_at_changes(&self, key: &H256) -> Option<H256> {
        if let Some(value) = self.storage_changes.get(key) {
            return Some(*value);
        }
        None
    }

    pub fn get_storage<B: DB>(&mut self, db: &mut B, key: &H256) -> Option<H256> {
        if let Some(value) = self.get_storage_at_changes(key) {
            return Some(value);
        }
        if let Some(value) = self.get_storage_at_backend(db, key) {
            return Some(value);
        }
        None
    }

    pub fn commit_storage<B: DB>(&mut self, db: &mut B) {
        let mut trie =  if self.storage_root == KECCAK_NULL_RLP {
            PatriciaTrie::new(db, RLPNodeCodec::default())
        } else {
            PatriciaTrie::from(db, RLPNodeCodec::default(), &self.storage_root.0).unwrap()
        };
        for (k, v) in self.storage_changes.drain() {
            if v.is_zero() {
                trie.remove(&k).unwrap();
            } else {
                trie.insert(&k, &v).unwrap();
            }
        }
        self.storage_root = trie.root().unwrap().into();
    }

    pub fn commit_code<B: DB>(&mut self, db: &mut B) {
        match (self.code_state == CodeState::Dirty, self.code.is_empty()) {
            (true, true) => {
                self.code_size = 0;
                self.code_state = CodeState::Clean;
            }
            (true, false) => {
                db.insert(&self.code_hash.clone(), &self.code).unwrap();
                self.code_size = self.code.len();
                self.code_state = CodeState::Clean;
            }
            (false, _) => {}
        }
    }

    pub fn clone_clean(&self) -> StateObject {
        StateObject {
            balance: self.balance.clone(),
            nonce: self.nonce.clone(),
            storage_root: self.storage_root.clone(),
            code: self.code.clone(),
            code_hash: self.code_hash.clone(),
            code_size: self.code_size.clone(),
            code_state: self.code_state.clone(),
            storage_changes: HashMap::new(),
        }
    }

    pub fn clone_dirty(&self) -> StateObject {
        let mut state_object = self.clone_clean();
        state_object.storage_changes = self.storage_changes.clone();
        state_object
    }

    pub fn merge(&mut self, other: StateObject) {
        self.balance = other.balance;
        self.nonce = other.nonce;
        self.storage_root = other.storage_root;
        self.code_hash = other.code_hash;
        self.code_state = other.code_state;
        self.code = other.code;
        self.code_size = other.code_size;
        self.storage_changes = other.storage_changes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

	#[test]
	fn state_object_new() {
        let o = StateObject::new(69u8.into(), 0u8.into());
        assert_eq!(o.balance(), 69u8.into());
        assert_eq!(o.nonce(), 0u8.into());
        assert_eq!(o.code_hash(), KECCAK_EMPTY);
		assert_eq!(o.storage_root, KECCAK_NULL_RLP);
		assert_eq!(hex::encode(rlp::encode(&o.account())), "f8448045a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470");
	}

    #[test]
    fn state_object_rlp() {
        let a = StateObject::new(69u8.into(), 0u8.into());
        let b = StateObject::from_rlp(&rlp::encode(&a.account())[..]);
        assert_eq!(a.balance(), b.balance());
		assert_eq!(a.nonce(), b.nonce());
		assert_eq!(a.code_hash(), b.code_hash());
		assert_eq!(a.storage_root, b.storage_root);
    }

    #[test]
    fn state_object_code() {
        let mut a = StateObject::new(69u8.into(), 0.into());
        let mut db = cita_trie::db::MemoryDB::new();
        a.init_code(vec![0x55, 0x44, 0xffu8]);
        assert_eq!(a.code_state, CodeState::Dirty);
        assert_eq!(a.code_size, 3);
        a.commit_code(&mut db);
        assert_eq!(a.code_state, CodeState::Clean);
        assert_eq!(a.code_hash, "af231e631776a517ca23125370d542873eca1fb4d613ed9b5d5335a46ae5b7eb".into());
        assert_eq!(db.get(&a.code_hash()).unwrap().unwrap(), vec![0x55, 0x44, 0xffu8]);
        a.init_code(vec![0x55]);
        assert_eq!(a.code_state, CodeState::Dirty);
        assert_eq!(a.code_size, 1);
        a.commit_code(&mut db);
        assert_eq!(a.code_hash, "37bf2238b11b68cdc8382cece82651b59d3c3988873b6e0f33d79694aa45f1be".into());
        assert_eq!(db.get(&a.code_hash()).unwrap().unwrap(), vec![0x55]);
    }

    #[test]
    fn state_object_storage_1() {
        let mut a = StateObject::new(69u8.into(), 0.into());
        let mut db = cita_trie::db::MemoryDB::new();
		a.set_storage(0.into(), 0x1234.into());
		a.commit_storage(&mut db);
		assert_eq!(a.storage_root, "ca8f89e4444c7453e96568511298af8049553232cfdb9255be8799d68c28b297".into());
    }

    #[test]
    #[ignore]
    fn state_object_storage_2() {
        let mut a = StateObject::new(69u8.into(), 0.into());
        let mut db = cita_trie::db::MemoryDB::new();
		a.set_storage(0.into(), 0x1234.into());
		a.commit_storage(&mut db);
        assert_eq!(a.storage_root, "ca8f89e4444c7453e96568511298af8049553232cfdb9255be8799d68c28b297".into());
		a.set_storage(1.into(), 0x1234.into());
		a.commit_storage(&mut db);
        assert_eq!(a.storage_root, "41cf81d2e6063cccd6965e9ca7d2b2ca95c6cf68012c9ac0be8564fd30e106b8".into());
		a.set_storage(1.into(), 0.into());
		a.commit_storage(&mut db);
		assert_eq!(a.storage_root, "ca8f89e4444c7453e96568511298af8049553232cfdb9255be8799d68c28b297".into());
    }

    #[test]
    fn state_object_storage_3() {
        let mut a = StateObject::new(69u8.into(), 0.into());
        let mut db = cita_trie::db::MemoryDB::new();
        let a_rlp = {
            a.set_storage(0x00u64.into(), 0x1234u64.into());
            a.commit_storage(&mut db);
            a.init_code(vec![]);
            a.commit_code(&mut db);
            rlp::encode(&a.account())
        };
		a =  StateObject::from_rlp(&a_rlp[..]);
		assert_eq!(a.storage_root, "ca8f89e4444c7453e96568511298af8049553232cfdb9255be8799d68c28b297".into());
		assert_eq!(a.get_storage(&mut db, &0x00u64.into()).unwrap(), 0x1234u64.into());
		assert_eq!(a.get_storage(&mut db, &0x01u64.into()), None);
    }


    #[test]
    fn state_object_note_code() {
        let mut a = StateObject::new(69u8.into(), 0.into());
        let mut db = cita_trie::db::MemoryDB::new();
		let a_rlp = {
			a.init_code(vec![0x55, 0x44, 0xffu8]);
			a.commit_code(&mut db);
			a.rlp()
		};
        a =  StateObject::from_rlp(&a_rlp[..]);
        a.read_code(&mut db);
		assert_eq!(a.code, vec![0x55, 0x44, 0xffu8]);
    }
}
