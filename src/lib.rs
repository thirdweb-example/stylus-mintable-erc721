#![cfg_attr(not(any(feature = "export-abi", test)), no_main)]
extern crate alloc;

use alloc::vec::Vec;
use alloy_primitives::{Address, FixedBytes, U256};
use alloy_sol_types::sol;
use stylus_sdk::{
    abi::Bytes,
    host::VM,
    prelude::*,
    storage::{StorageAddress, StorageBool, StorageMap},
};

const NATIVE_TOKEN_ADDRESS: Address = Address::new([
    0xEe, 0xee, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE, 0xeE
]);

// ERC-7201 storage slot for "token.minting.mintable.erc721"
// Calculated as: keccak256(abi.encode(uint256(keccak256("token.minting.mintable.erc721")) - 1)) & ~bytes32(uint256(0xff))
const MINTABLE_STORAGE_POSITION: U256 = U256::from_be_bytes([
    0x52, 0xc6, 0x32, 0x47, 0xe1, 0xf4, 0x7d, 0xb1, 0x9d, 0x5c, 0xe0, 0x46, 0x00, 0x30, 0xc4, 0x97,
    0xf0, 0x67, 0xca, 0x4c, 0xeb, 0xf7, 0xc4, 0x8b, 0x2d, 0x7e, 0x87, 0xa0, 0x4b, 0x07, 0xb6, 0x00
]);

const MINTER_ROLE: U256 = U256::from_limbs([1, 0, 0, 0]); // 1 << 0

sol_interface! {
    interface IOwnableRoles {
        function hasAllRoles(address user, uint256 roles) external view returns (bool);
    }
}

pub const ERROR_INCORRECT_NATIVE_TOKEN: u8 = 1;
pub const ERROR_REQUEST_OUT_OF_TIME: u8 = 2;
pub const ERROR_REQUEST_UID_REUSED: u8 = 3;
pub const ERROR_REQUEST_UNAUTHORIZED: u8 = 4;
pub const ERROR_SIGNATURE_MINT_UNAUTHORIZED: u8 = 5;

sol! {
    error MintableError(uint8 code);
}

#[derive(SolidityError)]
pub enum MintableErrors {
    MintableError(MintableError),
}

pub struct MintSignatureParamsERC721 {
    pub start_timestamp: u64,
    pub end_timestamp: u64,
    pub currency: Address,
    pub price_per_unit: U256,
    pub uid: FixedBytes<32>,
}

pub struct SaleConfig {
    pub primary_sale_recipient: Address,
}

pub struct CallbackFunction {
    pub selector: FixedBytes<4>,
}

pub struct FallbackFunction {
    pub selector: FixedBytes<4>,
    pub permission_bits: U256,
}

pub struct ModuleConfig {
    pub callback_functions: Vec<CallbackFunction>,
    pub fallback_functions: Vec<FallbackFunction>,
    pub required_interfaces: Vec<FixedBytes<4>>,
    pub register_installation_callback: bool,
}

struct MintableStorage {
    uid_used: StorageMap<FixedBytes<32>, StorageBool>,
    sale_config_primary_sale_recipient: StorageAddress,
}

impl MintableStorage {
    fn load(vm: &VM) -> Self {
        unsafe {
            Self {
                uid_used: StorageMap::new(MINTABLE_STORAGE_POSITION, 0, vm.clone()),
                sale_config_primary_sale_recipient: StorageAddress::new(MINTABLE_STORAGE_POSITION + U256::from(1), 0, vm.clone()),
            }
        }
    }
}

sol_storage! {
    #[entrypoint]
    pub struct StylusMintableERC721 {
    }
}

#[public]
impl StylusMintableERC721 {

    pub fn get_module_config(&self) -> (bool, Vec<FixedBytes<4>>, Vec<FixedBytes<4>>, Vec<FixedBytes<4>>, Vec<(FixedBytes<4>, U256)>) {
        let register_installation_callback = true;
        
        let required_interfaces = vec![
            FixedBytes::from([0x80, 0xac, 0x58, 0xcd]), // ERC721 interface
        ];
        
        let supported_interfaces = vec![];
        
        let callback_functions = vec![
            FixedBytes::from([0x8f, 0x3c, 0x6e, 0x41]), // beforeMintERC721 selector
            FixedBytes::from([0x1a, 0x2b, 0x3c, 0x4d]), // beforeMintWithSignatureERC721 selector
        ];
        
        let fallback_functions = vec![
            (FixedBytes::from([0x12, 0x34, 0x56, 0x78]), U256::ZERO), // getSaleConfig, no permission
            (FixedBytes::from([0x9a, 0xbc, 0xde, 0xf0]), U256::from(2)), // setSaleConfig, _MANAGER_ROLE
        ];
        
        (register_installation_callback, required_interfaces, supported_interfaces, callback_functions, fallback_functions)
    }

    pub fn on_install(&mut self, data: Bytes) -> Result<(), MintableErrors> {
        let primary_sale_recipient = Address::from_slice(&data[12..32]);
        MintableStorage::load(&self.vm()).sale_config_primary_sale_recipient.set(primary_sale_recipient);
        Ok(())
    }

    pub fn on_uninstall(&mut self, _data: Bytes) -> Result<(), MintableErrors> {
        Ok(())
    }

    pub fn before_mint_erc721(
        &mut self,
        _to: Address,
        _start_token_id: U256,
        _quantity: U256,
        _data: Bytes
    ) -> Result<Bytes, MintableErrors> {
        if !self.has_minter_role(self.vm().msg_sender()) {
            return Err(MintableErrors::MintableError(MintableError { code: ERROR_REQUEST_UNAUTHORIZED }));
        }
        Ok(Bytes(vec![].into()))
    }

    pub fn before_mint_with_signature_erc721(
        &mut self,
        _to: Address,
        _start_token_id: U256,
        quantity: U256,
        data: Bytes,
        signer: Address
    ) -> Result<Bytes, MintableErrors> {
        if !self.has_minter_role(signer) {
            return Err(MintableErrors::MintableError(MintableError { code: ERROR_SIGNATURE_MINT_UNAUTHORIZED }));
        }

        let params = self.decode_mint_signature_params(data)?;
        self.mint_with_signature_erc721(params)?;
        self.distribute_mint_price(self.vm().msg_sender(), params.2, quantity * params.3)?;
        
        Ok(Bytes(vec![].into()))
    }

    pub fn get_sale_config(&self) -> Address {
        MintableStorage::load(&self.vm()).sale_config_primary_sale_recipient.get()
    }

    pub fn set_sale_config(&mut self, primary_sale_recipient: Address) -> Result<(), MintableErrors> {
        MintableStorage::load(&self.vm()).sale_config_primary_sale_recipient.set(primary_sale_recipient);
        Ok(())
    }

    pub fn encode_bytes_on_install(&self, primary_sale_recipient: Address) -> Bytes {
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 12]);
        data.extend_from_slice(primary_sale_recipient.as_slice());
        Bytes(data.into())
    }

    pub fn encode_bytes_on_uninstall(&self) -> Bytes {
        Bytes(vec![].into())
    }

    pub fn encode_bytes_before_mint_with_signature_erc721(&self, start_timestamp: u64, end_timestamp: u64, currency: Address, price_per_unit: U256, uid: FixedBytes<32>) -> Bytes {
        let mut data = Vec::new();
        data.extend_from_slice(&start_timestamp.to_be_bytes());
        data.extend_from_slice(&end_timestamp.to_be_bytes());
        data.extend_from_slice(currency.as_slice());
        data.extend_from_slice(&price_per_unit.to_be_bytes::<32>());
        data.extend_from_slice(uid.as_slice());
        Bytes(data.into())
    }

    fn mint_with_signature_erc721(&mut self, params: (u64, u64, Address, U256, FixedBytes<32>)) -> Result<(), MintableErrors> {
        let (start_timestamp, end_timestamp, _currency, _price_per_unit, uid) = params;
        
        let now = self.vm().block_timestamp();
        if now < start_timestamp || end_timestamp <= now {
            return Err(MintableErrors::MintableError(MintableError { code: ERROR_REQUEST_OUT_OF_TIME }));
        }

        let mut storage = MintableStorage::load(&self.vm());
        if storage.uid_used.get(uid) {
            return Ok(()); // already used, skip
        }

        storage.uid_used.insert(uid, true);
        Ok(())
    }

    fn distribute_mint_price(&self, _owner: Address, currency: Address, price: U256) -> Result<(), MintableErrors> {
        if price == U256::ZERO {
            if self.vm().msg_value() > U256::ZERO {
                return Err(MintableErrors::MintableError(MintableError { code: ERROR_INCORRECT_NATIVE_TOKEN }));
            }
            return Ok(());
        }

        let sale_config = MintableStorage::load(&self.vm()).sale_config_primary_sale_recipient.get();

        if currency == NATIVE_TOKEN_ADDRESS {
            if self.vm().msg_value() != price {
                return Err(MintableErrors::MintableError(MintableError { code: ERROR_INCORRECT_NATIVE_TOKEN }));
            }
            // todo: transfer
            return Ok(());
        } else {
            if self.vm().msg_value() > U256::ZERO {
                return Err(MintableErrors::MintableError(MintableError { code: ERROR_INCORRECT_NATIVE_TOKEN }));
            }
            
            let transfer_sig = alloy_primitives::hex!("23b872dd");
            let mut data = Vec::new();
            data.extend_from_slice(&transfer_sig);
            data.extend_from_slice(_owner.as_slice());
            data.extend_from_slice(sale_config.as_slice());
            data.extend_from_slice(&price.to_be_bytes::<32>());
            
            // todo: transfer
            return Ok(());
        }

        Ok(())
    }

    fn decode_mint_signature_params(&self, data: Bytes) -> Result<(u64, u64, Address, U256, FixedBytes<32>), MintableErrors> {
        if data.len() < 104 {
            return Err(MintableErrors::MintableError(MintableError { code: ERROR_REQUEST_UNAUTHORIZED }));
        }

        let start_timestamp = u64::from_be_bytes([
            data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7]
        ]);
        let end_timestamp = u64::from_be_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15]
        ]);
        let currency = Address::from_slice(&data[28..48]);
        let price_per_unit = U256::from_be_slice(&data[48..80]);
        let uid = FixedBytes::<32>::from_slice(&data[72..104]);

        Ok((start_timestamp, end_timestamp, currency, price_per_unit, uid))
    }

    fn has_minter_role(&self, account: Address) -> bool {
        let ownable_roles = IOwnableRoles::from(self.vm().contract_address());
        match ownable_roles.has_all_roles(self.vm(), Call::new(), account, MINTER_ROLE) {
            Ok(result) => result,
            Err(_) => false,
        }
    }
}