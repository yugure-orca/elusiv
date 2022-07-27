use quote::quote;
use super::utils::{ upper_camel_to_upper_snake, named_sub_attribute };
use proc_macro2::TokenStream;

pub fn impl_elusiv_instruction(ast: &syn::DeriveInput) -> proc_macro2::TokenStream {
    let ast_ident = &ast.ident;

    let mut matches = quote!{};
    let mut len = quote!{};
    let mut functions = quote!{};
    let mut abi_functions = quote!{};

    if let syn::Data::Enum(e) = &ast.data {
        for (i, var) in e.variants.clone().iter().enumerate() {
            let ident = &var.ident;
            let name = upper_camel_to_upper_snake(&ident.to_string()).to_lowercase();
            let fn_name_abi: TokenStream = format!("{}_instruction", name).parse().unwrap();
            let fn_name: TokenStream = name.parse().unwrap();

            // Processor calls
            let mut accounts = quote!{};
            let mut fields = quote!{};
            let mut signature = quote!{};

            // Instruction creation
            let mut fields_with_type = quote!{};
            let mut user_accounts = quote!{};
            let mut instruction_accounts = quote!{};

            let mut var_size = quote!{};
            for field in &var.fields {
                let field_name = field.ident.clone().unwrap();
                let ty = field.ty.clone();

                fields.extend(quote! { #field_name, });
                fields_with_type.extend(quote! { #field_name: #ty, });

                if var_size.is_empty() {
                    var_size.extend(quote!{ <#ty>::SIZE })
                } else {
                    var_size.extend(quote!{ + <#ty>::SIZE })
                }
            }
            if var_size.is_empty() { var_size = quote!{ 0 } };
            let i = i as u8;
            len.extend(quote! {
                #i => { #var_size },
            });

            // Account attributes
            for (_, attr) in var.attrs.iter().enumerate() {
                let attr_name = attr.path.get_ident().unwrap().to_string();

                // Sub-attrs are the fields as in #[usr(sub_attr0 = .., sub_attr1, ..)]
                let mut fields = attr.tokens.to_string();
                fields.retain(|x| x != '{' && x != '}' && !x.is_whitespace());
                let sub_attrs: Vec<&str> = (&fields[1..fields.len() - 1]).split(',').collect();

                let mut account: TokenStream = sub_attrs[0].parse().unwrap();
                let mut account_init = Vec::new(); // used for creating the instruction objects with the abi-feature

                accounts.extend(quote! {
                    let #account = next_account_info(account_info_iter)?;    
                });

                // Signer check
                let is_signer = sub_attrs.contains(&"signer");
                if  is_signer {
                    accounts.extend(quote!{
                        if !#account.is_signer { return Err(InvalidArgument) }
                    });
                }

                // Writable check
                let is_writable= sub_attrs.contains(&"writable");
                if is_writable {
                    accounts.extend(quote!{
                        if !#account.is_writable { return Err(InvalidArgument) }
                    });
                }

                // Ownership check
                let is_owned= sub_attrs.contains(&"owned");
                if is_owned {
                    accounts.extend(quote!{
                        if #account.owner != program_id { return Err(InvalidArgument) }
                    });
                }

                // Ignore means not passing the account to the processor function
                let ignore = sub_attrs.contains(&"ignore");

                // `AccountInfo`?
                let as_account_info = sub_attrs.contains(&"account_info");

                let mut_token = if is_writable { quote!{ mut } } else { quote!{} };
                let account_init_fn = if is_writable { quote!{ new } } else { quote!{ new_readonly } };

                let user_account_type = if is_signer {
                    if is_writable { quote!{ WritableSignerAccount } } else { quote!{ SignerAccount } }
                } else if is_writable { quote!{ WritableUserAccount } } else { quote!{ UserAccount } };

                match attr_name.as_str() {
                    // `AccountInfo` (usage: <name>)
                    "acc" => {
                        user_accounts.extend(quote!{ #account: #user_account_type, });
                        account_init.push(quote!{
                            accounts.push(AccountMeta::#account_init_fn(#account.0, #is_signer));
                        });
                    }

                    // Program owned accounts that satisfy a pubkey constraint
                    "prg" => {
                        user_accounts.extend(quote!{ #account: #user_account_type, });
                        account_init.push(quote!{
                            accounts.push(AccountMeta::#account_init_fn(#account.0, #is_signer));
                        });

                        if !is_owned {
                            accounts.extend(quote!{
                                if #account.owner != program_id { return Err(InvalidArgument) }
                            });
                        }

                        if as_account_info {
                            let key: TokenStream = named_sub_attribute("key", sub_attrs[1]).parse().unwrap();

                            accounts.extend(quote!{
                                if #account.key.to_bytes() != #key { return Err(InvalidArgument) }
                            });

                            account = quote!{ &#account };
                        } else {
                            let ty = program_account_type(sub_attrs[1]);
                            let key: TokenStream = named_sub_attribute("key", sub_attrs[2]).parse().unwrap();

                            accounts.extend(quote!{
                                if #account.key.to_bytes() != #key { return Err(InvalidArgument) }
                                let acc_data = &mut #account.data.borrow_mut()[..];
                                let #mut_token #account = <#ty>::new(acc_data)?;
                            });

                            if is_writable {
                                account = quote!{ &mut #account };
                            } else {
                                account = quote!{ &#account };
                            }
                        }

                    }

                    // System program `AccountInfo` (usage: <name> <key = ..>)
                    "sys" => {
                        // Check that system program pubkey is correct (for this we have a field `key` that the pubkey gets compared to)
                        let key: TokenStream = named_sub_attribute("key", sub_attrs[1]).parse().unwrap();

                        accounts.extend(quote!{
                            if #key != *#account.key { return Err(InvalidArgument) };
                        });

                        account_init.push(quote!{
                            accounts.push(AccountMeta::#account_init_fn(#key, #is_signer));
                        });
                    }

                    // PDA accounts (usage: <name> <AccountType> <pda_offset: u64 = ..>? <account_info>? <multi_account>? <ownership>)
                    "pda" => {
                        // Every PDA account needs to implement the trait `elusiv::state::program_account::PDAAccount`
                        // - this trait allows us to verify PDAs
                        // - this allows us to define `MultiAccountAccount`s, which are a single main PDA account with `COUNT` sub-accounts
                        // - the seed of the main account plus the index of each sub-account is used to generate their PDAs

                        // The PDA account type
                        let ty = program_account_type(sub_attrs[1]);

                        // The PDA offset is an optional field, used to add an offset to the seed (e.g. to index of tree)
                        // - note: you can reference a field from an account added before this one as an offset as well
                        let pda_offset: TokenStream = if let Some(offset) = sub_attrs.get(2) {
                            if offset.starts_with("pda_offset") {
                                named_sub_attribute("pda_offset", offset).parse().unwrap()
                            } else { quote!{ None } }
                        } else { quote!{ None } };

                        // Multi account account
                        let multi_account = sub_attrs.contains(&"multi_accounts");

                        // (For multi accounts): skips all sub-accounts (-> no checks required -> speed up)
                        let ignore_sub_accounts = sub_attrs.contains(&"ignore_sub_accounts");

                        let skip_abi = sub_attrs.contains(&"skip_abi");
                        if skip_abi {
                            let offset_ident: TokenStream = format!("{}_pda_offset", sub_attrs[0]).parse().unwrap();
                            user_accounts.extend(quote!{ #offset_ident: Option<u64>, });
                            account_init.push(quote!{
                                accounts.push(AccountMeta::#account_init_fn(<#ty>::find(#offset_ident).0, #is_signer));
                            });
                        } else {
                            account_init.push(quote!{
                                accounts.push(AccountMeta::#account_init_fn(<#ty>::find(#pda_offset).0, #is_signer));
                            });
                        }

                        // PDA verification
                        let find_pda = sub_attrs.contains(&"find_pda"); // does not read the bump byte from the account data
                        if find_pda {
                            accounts.extend(quote!{
                                if <#ty>::find(#pda_offset).0 != *#account.key { return Err(InvalidArgument) }
                            });
                        } else {
                            accounts.extend(quote!{
                                if !<#ty>::is_valid_pubkey(&#account, #pda_offset, #account.key)? { return Err(InvalidArgument) }
                            });
                        }

                        if multi_account {
                            // Sub-accounts with PDA and ownership check for each
                            if !ignore_sub_accounts {
                                accounts.extend(quote!{
                                    let accounts = <#ty>::find_sub_accounts::<_, #ty, {<#ty>::COUNT}>(
                                        #account,
                                        program_id,
                                        #is_writable,
                                        account_info_iter,
                                    )?;
                                    let acc_data = &mut #account.data.borrow_mut()[..];
                                });

                                user_accounts.extend(quote!{ #account: &[#user_account_type], });
                                account_init.push(quote!{
                                    for account in #account {
                                        accounts.push(AccountMeta::#account_init_fn(account.0, #is_signer));
                                    }
                                });
                            } else {
                                accounts.extend(quote!{
                                    let acc_data = &mut #account.data.borrow_mut()[..];
                                    let mut accounts = std::collections::HashMap::new();
                                });
                            }

                            if as_account_info {
                                accounts.extend(quote!{
                                    accounts.insert(0, #account);
                                    let #account = accounts;
                                });
                                account = quote!{ #account };
                            } else if is_writable {
                                accounts.extend(quote!{ let mut #account = #ty::new(acc_data, accounts)?; });
                                account = quote!{ &mut #account };
                            } else {
                                accounts.extend(quote!{ let #account = #ty::new(acc_data, accounts)?; });
                                account = quote!{ &#account };
                            }
                        } else if as_account_info {
                            account = quote!{ &#account };
                        } else if is_writable {
                            accounts.extend(quote!{
                                let acc_data = &mut #account.data.borrow_mut()[..];
                                let #mut_token #account = <#ty>::new(acc_data)?;
                            });
                            account = quote!{ &mut #account };
                        } else {
                            accounts.extend(quote!{
                                let acc_data = &mut #account.data.borrow_mut()[..];
                                let #mut_token #account = <#ty>::new(acc_data)?;
                            });
                            account = quote!{ &#account };
                        }
                    },
                    v => panic!("Invalid attribute name {}", v)
                }

                // Add account to processor call signature
                if !ignore {
                    signature.extend(quote!{ #account, });
                }

                // Add account init
                instruction_accounts.extend(account_init.iter().fold(quote!{}, |acc, x| quote!{ #acc #x }));
            }

            matches.extend(quote! {
                #ast_ident::#ident { #fields } => {
                    Self::#fn_name(program_id, accounts, #fields)
                },
            });

            functions.extend(quote!{
                pub fn #fn_name(program_id: &Pubkey, accounts: &[AccountInfo], #fields_with_type) -> ProgramResult {
                    let mut account_info_iter = &mut accounts.iter();
                    #accounts
                    processor::#fn_name(#signature #fields)
                }
            });

            abi_functions.extend(quote!{
                pub fn #fn_name_abi(#fields_with_type #user_accounts) -> solana_program::instruction::Instruction {
                    let mut accounts = Vec::new();

                    #instruction_accounts
                    let data = #ast_ident::#ident { #fields };
                    let data = ElusivInstruction::try_to_vec(&data).unwrap();

                    solana_program::instruction::Instruction::new_with_bytes(
                        crate::id(),
                        &data,
                        accounts,
                    )
                }
            });
        }

        quote! {
            impl #ast_ident {
                pub fn process(program_id: &Pubkey, accounts: &[AccountInfo], instruction: #ast_ident) -> ProgramResult {
                    match instruction {
                        #matches
                        _ => { Err(InvalidInstructionData) }
                    }
                }

                #functions

                pub fn len(variant_index: u8) -> usize {
                    match variant_index {
                        #len
                        _ => { 0 }
                    }
                }
            }
    
            #[cfg(feature = "instruction-abi")]
            impl #ast_ident {
                #abi_functions
            }
    
        }
    } else { panic!("Only enums can be instructions") }
}

fn program_account_type(name: &str) -> TokenStream {
    (String::from(name) + "Account").parse().unwrap()
}