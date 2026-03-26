#![cfg_attr(not(any(feature = "export-abi", test)), no_main)]
extern crate alloc;

use alloc::vec::Vec;
use alloy_primitives::{Address, FixedBytes, U256};
use alloy_sol_types::sol;
use stylus_sdk::{
    abi::Bytes,
    call::RawCall,
    function_selector,
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


pub struct SaleConfig {
    pub primarySaleRecipient: Address,
}

sol! {
    #[derive(Debug, AbiType)]
    struct CallbackFunction {
        bytes4 selector;
    }

    #[derive(Debug, AbiType)]
    struct FallbackFunction {
        bytes4 selector;
        uint256 permissionBits;
    }

    #[derive(Debug, AbiType)]
    struct ModuleConfig {
        bool registerInstallationCallback;
        bytes4[] requiredInterfaces;
        bytes4[] supportedInterfaces;
        CallbackFunction[] callbackFunctions;
        FallbackFunction[] fallbackFunctions;
    }
}

struct MintableStorage {
    uid_used: StorageMap<FixedBytes<32>, StorageBool>,
    sale_config_primarySaleRecipient: StorageAddress,
}

impl MintableStorage {
    fn load(vm: &VM) -> Self {
        unsafe {
            Self {
                uid_used: StorageMap::new(MINTABLE_STORAGE_POSITION, 0, vm.clone()),
                sale_config_primarySaleRecipient: StorageAddress::new(MINTABLE_STORAGE_POSITION + U256::from(1), 0, vm.clone()),
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
    #[constructor]
    pub fn constructor(&mut self) -> Result<(), String> {
        Ok(())
    }

    pub fn get_module_config(&self) -> Result<ModuleConfig, Vec<u8>> {
        Ok(ModuleConfig {
            registerInstallationCallback: true,
            requiredInterfaces: vec![
                FixedBytes::from([0x80, 0xac, 0x58, 0xcd]), // ERC721 interface
            ],
            supportedInterfaces: vec![],
            callbackFunctions: vec![
                CallbackFunction {
                    selector: FixedBytes::from(function_selector!("beforeMintERC721", Address, U256, U256, Bytes)),
                },
            ],
            fallbackFunctions: vec![
                FallbackFunction {
                    selector: FixedBytes::from(function_selector!("getSaleConfig")),
                    permissionBits: U256::ZERO,
                },
                FallbackFunction {
                    selector: FixedBytes::from(function_selector!("setSaleConfig", Address)),
                    permissionBits: U256::from(2), // _MANAGER_ROLE
                },
            ],
        })
    }

    pub fn on_install(&mut self, data: Bytes) -> Result<(), String> {
        let primarySaleRecipient = Address::from_slice(&data[12..32]);
        MintableStorage::load(&self.vm()).sale_config_primarySaleRecipient.set(primarySaleRecipient);
        Ok(())
    }

    pub fn on_uninstall(&mut self, _data: Bytes) -> Result<(), String> {
        Ok(())
    }

    #[selector(name = "beforeMintERC721")]
    pub fn before_mint_erc721(
        &mut self,
        _to: Address,
        _start_token_id: U256,
        _quantity: U256,
        _data: Bytes
    ) -> Result<Bytes, String> {
        if !self.has_minter_role(self.vm().msg_sender()) {
            return Err("Not authorized".into());
        }
        Ok(Bytes(vec![].into()))
    }


    pub fn get_sale_config(&self) -> Address {
        MintableStorage::load(&self.vm()).sale_config_primarySaleRecipient.get()
    }

    pub fn set_sale_config(&mut self, primarySaleRecipient: Address) -> Result<(), String> {
        MintableStorage::load(&self.vm()).sale_config_primarySaleRecipient.set(primarySaleRecipient);
        Ok(())
    }

    pub fn encode_bytes_on_install(&self, primarySaleRecipient: Address) -> Bytes {
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 12]);
        data.extend_from_slice(primarySaleRecipient.as_slice());
        Bytes(data.into())
    }

    pub fn encode_bytes_on_uninstall(&self) -> Bytes {
        Bytes(vec![].into())
    }

    fn distribute_mint_price(&self, _owner: Address, currency: Address, price: U256) -> Result<(), String> {
        if price == U256::ZERO {
            if self.vm().msg_value() > U256::ZERO {
                return Err("Incorrect native token".into());
            }
            return Ok(());
        }

        let sale_config = MintableStorage::load(&self.vm()).sale_config_primarySaleRecipient.get();

        if currency == NATIVE_TOKEN_ADDRESS {
            if self.vm().msg_value() != price {
                return Err("Incorrect native token".into());
            }
            unsafe {
                RawCall::new_with_value(&*self.vm(), price)
                    .call(sale_config, &[])
                    .map_err(|_| String::from("Native transfer failed"))?;
            }
            return Ok(());
        } else {
            if self.vm().msg_value() > U256::ZERO {
                return Err("Incorrect native token".into());
            }

            let transfer_sig = alloy_primitives::hex!("23b872dd");
            let mut data = Vec::new();
            data.extend_from_slice(&transfer_sig);
            data.extend_from_slice(&[0u8; 12]);
            data.extend_from_slice(_owner.as_slice());
            data.extend_from_slice(&[0u8; 12]);
            data.extend_from_slice(sale_config.as_slice());
            data.extend_from_slice(&price.to_be_bytes::<32>());

            unsafe {
                RawCall::new(&*self.vm())
                    .call(currency, &data)
                    .map_err(|_| String::from("ERC20 transferFrom failed"))?;
            }
            return Ok(());
        }
    }


    fn has_minter_role(&self, account: Address) -> bool {
        let ownable_roles = IOwnableRoles::from(self.vm().contract_address());
        match ownable_roles.has_all_roles(self.vm(), Call::new(), account, MINTER_ROLE) {
            Ok(result) => result,
            Err(_) => false,
        }
    }
}