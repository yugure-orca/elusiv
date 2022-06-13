//! Traits used to represent types of accounts, owned by the program

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::account_info::AccountInfo;
use solana_program::program_error::ProgramError;
use solana_program::pubkey::Pubkey;

use crate::macros::BorshSerDeSized;
use crate::bytes::{BorshSerDeSized, u64_as_usize_safe};
use crate::types::U256;

pub trait SizedAccount {
    const SIZE: usize;
}

pub trait ProgramAccount<'a>: SizedAccount {
    type T: SizedAccount;

    fn new(d: &'a mut [u8]) -> Result<Self::T, ProgramError>;
}

pub trait MultiAccountProgramAccount<'a, 'b, 't>: SizedAccount {
    type T: SizedAccount;

    fn new(d: &'a mut [u8], accounts: Vec<&'b AccountInfo<'t>>) -> Result<Self::T, ProgramError>;
}

/// This trait is used by the `elusiv_instruction` and `elusiv_accounts` macros
/// - a PDAAccount is simply a PDA with:
///     1. the leading fields specified by `PDAAccountFields`
///     2. a PDA that is derived using the following seed: `&[ &SEED, offset?, bump ]`
/// - so there are two kinds of PDAAccounts:
///     - single instance: the pda_offset is `None` -> `&[ &SEED, bump ]`
///     - multi instance: the pda_offset is `Some(offset)` -> `&[ &SEED, offset, bump ]`
pub trait PDAAccount {
    const SEED: &'static [u8];

    fn pda_bump_seed(&self) -> u8;
    fn pda_version(&self) -> u8;
    fn pda_initialized(&self) -> bool;

    fn set_pda_initialized(&mut self, initialized: bool);

    fn find(offset: Option<u64>) -> (Pubkey, u8) {
        let seed = Self::offset_seed(offset);
        let seed: Vec<&[u8]> = seed.iter().map(|x| &x[..]).collect();

        Pubkey::find_program_address(&seed, &crate::id())
    }

    fn pubkey(offset: Option<u64>, bump: u8) -> Result<Pubkey, ProgramError> {
        let mut seed = Self::offset_seed(offset);
        seed.push(vec![bump]);
        let seed: Vec<&[u8]> = seed.iter().map(|x| &x[..]).collect();

        match Pubkey::create_program_address(&seed, &crate::id()) {
            Ok(v) => Ok(v),
            Err(_) => Err(ProgramError::InvalidSeeds)
        }
    }

    fn offset_seed(offset: Option<u64>) -> Vec<Vec<u8>> {
        match offset {
            Some(offset) => vec![Self::SEED.to_vec(), offset.to_le_bytes().to_vec()],
            None => vec![Self::SEED.to_vec()]
        }
    }

    fn is_valid_pubkey(account: &AccountInfo, offset: Option<u64>, pubkey: &Pubkey) -> Result<bool, ProgramError> {
        let acc_data = &account.data.borrow()[..PDAAccountFields::SIZE];
        match PDAAccountFields::new(acc_data) {
            Ok(a) => Ok(Self::pubkey(offset, a.bump_seed)? == *pubkey),
            Err(_) => Err(ProgramError::InvalidAccountData)
        }
    }
} 

/// Every `PDAAccount` has these fields as the first fields. (guaranteed by the `elusiv_account` macro) 
#[derive(BorshDeserialize, BorshSerialize, BorshSerDeSized)]
pub struct PDAAccountFields {
    /// Saves us SHA computations
    pub bump_seed: u8,

    /// Used for future account migrations
    pub version: u8,

    /// In general useless, only if an account-type (like the `MultiAccountAccount` uses it)
    /// by default: false
    pub initialized: bool,
}

impl PDAAccountFields {
    pub fn new(data: &[u8]) -> Result<Self, std::io::Error> {
        PDAAccountFields::try_from_slice(&data[..Self::SIZE])
    }
}

/// Every `MultiAccountAccount` has these fields at the beginning. (guaranteed by the `elusiv_account` macro) 
#[derive(BorshDeserialize, BorshSerialize, BorshSerDeSized)]
pub struct MultiAccountAccountFields<const COUNT: usize> {
    pub bump_seed: u8,
    pub version: u8,
    pub initialized: bool,

    pub pubkeys: [U256; COUNT],
}

impl<const COUNT: usize> MultiAccountAccountFields<COUNT> {
    pub fn new(data: &[u8]) -> Result<Self, std::io::Error> {
        MultiAccountAccountFields::try_from_slice(&data[..Self::SIZE])
    }

    pub fn all_pubkeys(&self) -> Vec<Pubkey> {
        self.pubkeys.iter().map(|x| Pubkey::new(x)).collect()
    }
}

/// Certain accounts, like the `VerificationAccount` can be instantiated multiple times.
/// - this allows for parallel computations/usage
/// - so we can compare this index with `MAX_INSTANCES` to check validity
pub trait MultiInstancePDAAccount: PDAAccount {
    const MAX_INSTANCES: u64;

    fn is_valid(&self, index: u64) -> bool {
        index < Self::MAX_INSTANCES
    }
}

// https://github.com/solana-labs/solana/blob/3608801a54600431720b37b53d7dbf88de4ead24/sdk/program/src/system_instruction.rs#L142
pub use solana_program::system_instruction::MAX_PERMITTED_DATA_LENGTH; // 10 MiB

/// Allows for storing data across multiple accounts (needed for data sized >10 MiB)
/// - these accounts can be PDAs, but will most likely be data accounts (size > 10 KiB)
/// - by default all these accounts are assumed to have the same size = `INTERMEDIARY_ACCOUNT_SIZE`
pub trait MultiAccountAccount<'t>: PDAAccount {
    /// The count of subsidiary accounts
    const COUNT: usize;

    /// The size an intermediary account has (not last account)
    const INTERMEDIARY_ACCOUNT_SIZE: usize;

    /// Returns a slice of length `Self::COUNT` containing all pubkeys of the sub-accounts
    fn get_all_pubkeys(&self) -> Vec<U256>;

    /// Set the `Self::COUNT` sub-accounts pubkeys
    fn set_all_pubkeys(&mut self, pubkeys: &[U256]);

    fn get_account(&self, account_index: usize) -> &AccountInfo<'t>;
}

macro_rules! data_slice {
    ($id: ident, $self: ident, $index: ident) => {
        let (account_index, local_index) = $self.account_and_local_index($index);
        let account = $self.get_account(account_index);
        let data = &mut account.data.borrow_mut()[..];
        let $id = &mut data[local_index * Self::T::SIZE..(local_index + 1) * Self::T::SIZE];
    };
}

/// A `MultiAccountAccount` for which all sub-accounts but the last one have `INTERMEDIARY_ACCOUNT_SIZE`
pub trait HeterogenMultiAccountAccount<'a>: MultiAccountAccount<'a> {
    const LAST_ACCOUNT_SIZE: usize;
}

/// Allows for storing data in an array that cannot be stored in a single Solana account
/// - `BigArrayAccount` takes care of parsing the data stored in those accounts
/// - these accounts are normal data accounts generated by extending the `BigArrayAccount`'s pda_seed
pub trait BigArrayAccount<'a>: MultiAccountAccount<'a> {
    type T: BorshSerDeSized;

    const VALUES_COUNT: usize;
    const MAX_VALUES_PER_ACCOUNT: usize = u64_as_usize_safe(MAX_PERMITTED_DATA_LENGTH) / Self::T::SIZE;

    // indices in this implementation are always the external array indices and not byte-indices!
    fn account_and_local_index(&self, index: usize) -> (usize, usize) {
        let account_index = index / Self::MAX_VALUES_PER_ACCOUNT;
        (account_index, index % Self::MAX_VALUES_PER_ACCOUNT)
    }

    // Returns the value at `index` from the correct sub-account
    fn get(&self, index: usize) -> Self::T {
        data_slice!(data, self, index);
        Self::T::try_from_slice(data).unwrap()
    }

    fn set(&self, index: usize, value: Self::T) {
        data_slice!(data, self, index);
        Self::T::override_slice(&value, data);
    }
}

impl<'a, T: BigArrayAccount<'a, T=N>, N: BorshSerDeSized> HeterogenMultiAccountAccount<'a> for T {
    const LAST_ACCOUNT_SIZE: usize = (Self::VALUES_COUNT - (Self::COUNT - 1) * Self::MAX_VALUES_PER_ACCOUNT) * N::SIZE;
}

pub const fn max_account_size(element_size: usize) -> usize {
    (u64_as_usize_safe(MAX_PERMITTED_DATA_LENGTH) / element_size) * element_size
}

pub const fn big_array_accounts_count(len: usize, element_size: usize) -> usize {
    let max = u64_as_usize_safe(MAX_PERMITTED_DATA_LENGTH) / element_size;
    len / max + (if len % max == 0 { 0 } else { 1 })
}

pub const fn get_multi_accounts_count(max_elements_per_account: usize, elements_count: usize) -> usize {
    let count = elements_count / max_elements_per_account;
    count + (if elements_count % max_elements_per_account == 0 { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macros::account;

    const SEED: &[u8] = b"TEST_seed";

    struct TestPDAAccount { }
    impl PDAAccount for TestPDAAccount {
        const SEED: &'static [u8] = SEED;

        fn pda_bump_seed(&self) -> u8 { 0 }
        fn pda_version(&self) -> u8 { 0 }
        fn pda_initialized(&self) -> bool { false }
        fn set_pda_initialized(&mut self, _initialized: bool) {}
    }

    #[test]
    /// Test that the pubkeys and bumps generated by the `PDAAccount` trait are correct
    fn test_pda_account() {
        let offset = Some(12);
        let offset_bytes = u64::to_le_bytes(offset.unwrap());
        let seed = vec![SEED, &offset_bytes[..]];

        let (expected_pubkey, expected_bump) = Pubkey::find_program_address(&seed, &crate::id());
        let (result_pubkey, result_bump) = TestPDAAccount::find(offset);

        assert_eq!(TestPDAAccount::offset_seed(offset), seed);
        assert_eq!(result_pubkey, expected_pubkey);
        assert_eq!(result_bump, expected_bump);
        assert_eq!(TestPDAAccount::pubkey(offset, result_bump).unwrap(), expected_pubkey);
    }

    const MAX_VALUES_PER_ACCOUNT: usize = max_account_size(32) / 32;
    const ELEMENTS_COUNT: usize = MAX_VALUES_PER_ACCOUNT * 3 + 1;
    const COUNT: usize = big_array_accounts_count(ELEMENTS_COUNT, 32);
    const LAST_ACCOUNT_SIZE: usize = ELEMENTS_COUNT * 32 - (COUNT - 1) * (MAX_VALUES_PER_ACCOUNT * 32);

    struct TestMultiAccountAccount<'t> {
        pubkeys: [U256; COUNT],
        accounts: [AccountInfo<'t>; COUNT],
    }
    impl<'t> PDAAccount for TestMultiAccountAccount<'t> {
        const SEED: &'static [u8] = b"ABCDEFG";

        fn pda_bump_seed(&self) -> u8 { 0 }
        fn pda_version(&self) -> u8 { 0 }
        fn pda_initialized(&self) -> bool { false }
        fn set_pda_initialized(&mut self, _initialized: bool) {}
    }
    impl<'t> MultiAccountAccount<'t> for TestMultiAccountAccount<'t> {
        const COUNT: usize = COUNT;
        const INTERMEDIARY_ACCOUNT_SIZE: usize = max_account_size(32);

        fn get_all_pubkeys(&self) -> Vec<U256> {
            self.pubkeys.to_vec()
        }

        fn set_all_pubkeys(&mut self, pubkeys: &[U256]) {
            assert!(pubkeys.len() == Self::COUNT);
            self.pubkeys[..Self::COUNT].copy_from_slice(&pubkeys[..Self::COUNT]);
        }

        fn get_account(&self, account_index: usize) -> &AccountInfo<'t> {
            &self.accounts[account_index]
        }
    }

    #[test]
    fn test_multi_account_account() {
        assert_eq!(TestMultiAccountAccount::COUNT, 4);

        let pk0 = Pubkey::new_unique();
        let pk1 = Pubkey::new_unique();
        let pk2 = Pubkey::new_unique();
        let pk3 = Pubkey::new_unique();

        account!(acc0, pk0, vec![0; TestMultiAccountAccount::INTERMEDIARY_ACCOUNT_SIZE]);
        account!(acc1, pk1, vec![0; TestMultiAccountAccount::INTERMEDIARY_ACCOUNT_SIZE]);
        account!(acc2, pk2, vec![0; TestMultiAccountAccount::INTERMEDIARY_ACCOUNT_SIZE]);
        account!(acc3, pk3, vec![0; LAST_ACCOUNT_SIZE]);

        let acc = TestMultiAccountAccount {
            pubkeys: [ acc0.key.to_bytes(), acc1.key.to_bytes(), acc2.key.to_bytes(), acc3.key.to_bytes(), ],
            accounts: [ acc0.clone(), acc1.clone(), acc2.clone(), acc3.clone() ]
        };

        assert_eq!(acc.get_all_pubkeys(), [ acc0.key.to_bytes(), acc1.key.to_bytes(), acc2.key.to_bytes(), acc3.key.to_bytes() ].to_vec());
    }

    impl<'t> BigArrayAccount<'t> for TestMultiAccountAccount<'t> {
        type T = U256;
        const VALUES_COUNT: usize = ELEMENTS_COUNT;
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn test_big_array_account() {
        assert_eq!(LAST_ACCOUNT_SIZE, TestMultiAccountAccount::LAST_ACCOUNT_SIZE);

        let mut acc0_data = vec![0; TestMultiAccountAccount::INTERMEDIARY_ACCOUNT_SIZE];
        for i in 0..32 { acc0_data[1024 * 32 + i] = 1; }

        let mut acc1_data = vec![0; TestMultiAccountAccount::INTERMEDIARY_ACCOUNT_SIZE];
        for i in 0..32 { acc1_data[i] = 2; }

        let mut acc2_data = vec![0; TestMultiAccountAccount::INTERMEDIARY_ACCOUNT_SIZE];
        for i in 0..32 { acc2_data[333 * 32 + i] = 3; }

        let mut acc3_data = vec![0; TestMultiAccountAccount::LAST_ACCOUNT_SIZE];
        for i in 0..32 { acc3_data[LAST_ACCOUNT_SIZE - 1 - i] = 6; }

        let pk0 = Pubkey::new_unique();
        let pk1 = Pubkey::new_unique();
        let pk2 = Pubkey::new_unique();
        let pk3 = Pubkey::new_unique();

        account!(acc0, pk0, acc0_data);
        account!(acc1, pk1, acc1_data);
        account!(acc2, pk2, acc2_data);
        account!(acc3, pk3, acc3_data);

        let acc = TestMultiAccountAccount {
            pubkeys: [ acc0.key.to_bytes(), acc1.key.to_bytes(), acc2.key.to_bytes(), acc3.key.to_bytes(), ],
            accounts: [ acc0.clone(), acc1.clone(), acc2.clone(), acc3.clone() ]
        };

        // Getting over the border of multiple accounts
        assert_eq!(acc.get(0), [0; 32]);
        assert_eq!(acc.get(1024), [1; 32]);
        assert_eq!(acc.get(MAX_VALUES_PER_ACCOUNT), [2; 32]);
        assert_eq!(acc.get(MAX_VALUES_PER_ACCOUNT * 2 + 333), [3; 32]);
        assert_eq!(acc.get(TestMultiAccountAccount::VALUES_COUNT - 1), [6; 32]); // last element

        // Setting
        acc.set(MAX_VALUES_PER_ACCOUNT, [5; 32]);
        assert_eq!(acc.get(MAX_VALUES_PER_ACCOUNT), [5; 32]);
    }

    #[test]
    fn test_max_account_size() {
        assert_eq!(max_account_size(1) as u64, MAX_PERMITTED_DATA_LENGTH);
        assert_eq!(max_account_size(2) as u64, MAX_PERMITTED_DATA_LENGTH);
        assert_eq!(max_account_size(3) as u64, MAX_PERMITTED_DATA_LENGTH - 1);
    }

    #[test]
    fn test_big_array_accounts_count() {
        assert_eq!(big_array_accounts_count(MAX_PERMITTED_DATA_LENGTH as usize, 1), 1);
        assert_eq!(big_array_accounts_count(MAX_PERMITTED_DATA_LENGTH as usize, 32), 32);
        assert_eq!(big_array_accounts_count(MAX_PERMITTED_DATA_LENGTH as usize + 1, 32), 33);
    }

    #[test]
    fn test_get_multi_accounts_count() {
        assert_eq!(get_multi_accounts_count(32, 100), 4);
    }
}