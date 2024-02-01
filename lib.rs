#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[frame_support::pallet(dev_mode)]
pub mod pallet {
    use frame_support::{
        pallet_prelude::*, traits::{Currency, Get, Randomness}
    };
    use frame_system::pallet_prelude::*;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        type Currency: Currency<Self::AccountId>;
        type Randomness: Randomness<Self::Hash, BlockNumberFor<Self>>;

        #[pallet::constant]
        type MaximumOwned: Get<u32>;
    }

    #[derive(Clone, Encode, Decode, PartialEq, Copy, RuntimeDebug, TypeInfo, MaxEncodedLen)]
    pub enum Color {
        Red,
        Yellow,
        Blue,
        Green
    }

    type BalanceOf<T> =
    <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

    #[derive(Clone, Encode, Decode, PartialEq, Copy, RuntimeDebug, TypeInfo, MaxEncodedLen)]
    #[scale_info(skip_type_params(T))]
    pub struct Collectible<T: Config> {
        // Unsigned integers of 16 bytes to represent a unique identifier
        pub unique_id: [u8; 16],
        // `None` assumes not for sale
        pub price: Option<BalanceOf<T>>,
        pub color: Color,
        pub owner: T::AccountId,
    }

    #[pallet::storage]
    pub(super) type CollectiblesCount<T:Config> = StorageValue<_,u64,ValueQuery>;

    /// Maps the Collectible struct to the unique_id.
    #[pallet::storage]
    pub(super) type CollectibleMap<T: Config> = StorageMap<_, Twox64Concat, [u8; 16], Collectible<T>>;

    /// Track the collectibles owned by each account.
    #[pallet::storage]
    pub(super) type OwnerOfCollectibles<T: Config> = StorageMap<
        _,
        Twox64Concat,
        T::AccountId,
        BoundedVec<[u8; 16], T::MaximumOwned>,
        ValueQuery,
    >;


    #[pallet::error]
    pub enum Error<T> {
        /// Each collectible must have a unique identifier
        DuplicateCollectible,
        /// An account can't exceed the `MaximumOwned` constant
        MaximumCollectiblesOwned,
        /// The total supply of collectibles can't exceed the u64 limit
        BoundsOverflow,
        /// The collectible doesn't exist
        NoCollectible,
        /// You are not the owner
        NotOwner,
        /// Trying to transfer a collectible to yourself
        TransferToSelf,
        /// Error sent when trying to buy or get/remove the price of a collectible which's not on sale
        CollectibleNotForSale,
        /// Error sent if trying to buy a collectible under its price
        OfferedPriceTooLow
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A new collectible was successfully created
        CollectibleCreated { collectible: [u8; 16], owner: T::AccountId },
        /// A collectible was successfully transferred.
        TransferSucceeded { from: T::AccountId, to: T::AccountId, collectible: [u8; 16] },
        /// A collectible's owner has set a price for it
        PriceSet { collectible: [u8;16], price: BalanceOf<T> },
        /// A collectible's owner has retired it from the market
        NotLongerOnSale { collectible: [u8;16] },
        /// A purchase occured
        Sold { seller: T::AccountId, buyer: T::AccountId, collectible: [u8;16], price: BalanceOf<T>},
        /// A collectible's been destroyed
        CollectibleDestroyed { collectible: [u8;16] }
    }


    impl<T:Config> Pallet<T>{
        fn gen_unique_id() -> ([u8;16], Color){
            let random = T::Randomness::random(&b"unique_id"[..]).0;

            let unique_payload = (
                random,
                frame_system::Pallet::<T>::extrinsic_index().unwrap_or_default(),
                frame_system::Pallet::<T>::block_number()
            );

            let encode_payload = unique_payload.encode();
            let hash = frame_support::Hashable::blake2_128(&encode_payload);
            if hash[0]%2 == 0 {
                (hash, Color::Red)
            }
            else{
                (hash, Color::Yellow)
            }
        }

        fn mint(
            owner: &T::AccountId,
            unique_id: [u8;16],
            color: Color
        ) -> Result<[u8;16],DispatchError>{
            let collectible = Collectible::<T> {
                unique_id,
                price: None,
                color,
                owner: owner.clone()
            };

            ensure!(!CollectibleMap::<T>::contains_key(&unique_id), Error::<T>::DuplicateCollectible);
            let count = CollectiblesCount::<T>::get();
            let new_count = count.checked_add(1).ok_or(Error::<T>::BoundsOverflow)?;

            OwnerOfCollectibles::<T>::try_append(&owner, unique_id)
                .map_err(|_| Error::<T>::MaximumCollectiblesOwned)?;

            CollectibleMap::<T>::insert(unique_id, collectible);
            CollectiblesCount::<T>::put(new_count);

            Self::deposit_event(Event::CollectibleCreated { collectible: unique_id, owner: owner.clone() });

            Ok(unique_id)
        }

        // Update storage to transfer collectible
        pub fn do_transfer(
            collectible_id: [u8; 16],
            to: T::AccountId,
        ) -> DispatchResult {
            let (collectible, from, from_collection, to_collection) = Self::pre_transfer(collectible_id, &to)?;
            Self::post_transfer(&collectible, &from, &to, from_collection, to_collection);		
            Self::deposit_event(Event::TransferSucceeded { from, to, collectible: collectible_id });
            Ok(())
        }

        pub fn do_buy(
            collectible_id: [u8; 16],
            buyer: T::AccountId,
            price: BalanceOf<T>
        ) -> DispatchResult{
            let (collectible, seller, seller_collection, buyer_collection) = Self::pre_transfer(collectible_id, &buyer)?;
            // Nothing can fail after the balance transfer, so this is the latest point where we can return an error. After that, it's enoguh with updating the storage
            T::Currency::transfer(&buyer, &seller, price, frame_support::traits::tokens::ExistenceRequirement::KeepAlive)?;
            // Update storage
            Self::post_transfer(&collectible, &seller, &buyer, seller_collection, buyer_collection);
            Self::deposit_event(Event::Sold{ seller, buyer , collectible: collectible_id, price});      
            Ok(())
        }

        /// This function encapsulates all the logic needed before a transfer/purchase
        fn pre_transfer(
            collectible_id: [u8; 16],
            to: &T::AccountId
        ) -> Result<
            (
                Collectible<T>, 
                T::AccountId, 
                BoundedVec<[u8; 16], T::MaximumOwned>, 
                BoundedVec<[u8; 16], T::MaximumOwned>
            ), Error<T>
        >{
            let mut collectible = CollectibleMap::<T>::get(&collectible_id).unwrap(); // Collectible exists is already checked in the callable functions
            let from = collectible.owner;
            // Ensure the collectible isn't sent to its owner
            ensure!(from != *to, Error::<T>::TransferToSelf);
            // Retrieve the collection of 'from' and 'to'
            let mut from_collection = OwnerOfCollectibles::<T>::get(&from);
            let mut  to_collection = OwnerOfCollectibles::<T>::get(&to);

            // Remove the collectible from the 'from' collection
            if let Some(index) = from_collection.iter().position(|&element| element == collectible_id){
                from_collection.swap_remove(index);
            } // Cannot be None if everything is well implemented, as we know this account owns the collectible due to the previous lines

            // Try pushing the collectible into the to collection
            to_collection.try_push(collectible_id).map_err(|_| Error::<T>::MaximumCollectiblesOwned)?;

            collectible.owner = to.clone();
            collectible.price = None; // After transfer, the token isn't in sale, its new owner must set the desired price if wishing to sell it

            Ok((collectible, from, from_collection, to_collection))
        }

        // This function updates storage after every transfer/purchase
        fn post_transfer(
            collectible: &Collectible<T>,
            from: &T::AccountId,
            to: &T::AccountId,
            from_collection: BoundedVec<[u8; 16], T::MaximumOwned>,
            to_collection: BoundedVec<[u8; 16], T::MaximumOwned>
        ){
            // Write updates to storage
            CollectibleMap::<T>::insert(collectible.unique_id, collectible);
            OwnerOfCollectibles::<T>::insert(from, from_collection);
            OwnerOfCollectibles::<T>::insert(to, to_collection);
        }
    }

    #[pallet::call]
    impl<T:Config> Pallet<T>{
        #[pallet::weight(0)]
        pub fn create_collectible(origin: OriginFor<T>) -> DispatchResult{
            let sender = ensure_signed(origin)?;

            let (unique_id, color) = Self::gen_unique_id();

            Self::mint(&sender, unique_id, color)?;

            Ok(())
        }

        #[pallet::weight(0)] // Planning to update this to avoid the possibility of this dispatchable being called in the same block of a transfer/buy, but I still don't know how to do it correctly :(
        pub fn destroy_collectible(
            origin: OriginFor<T>,
            collectible_id: [u8; 16]
        ) -> DispatchResult{
            let sender = ensure_signed(origin)?;

            let collectible = CollectibleMap::<T>::get(&collectible_id).ok_or(Error::<T>::NoCollectible)?;
            ensure!(collectible.owner == sender, Error::<T>::NotOwner);

            let count = CollectiblesCount::<T>::get();
            CollectiblesCount::<T>::put(count-1); // No risk of underflow as this collectible indeed exists, so count is at least 1.

            // Remove the collectible from the map
            CollectibleMap::<T>::remove(&collectible_id);

            // Remove the collectible from the 'sender' collection
            let mut sender_collection = OwnerOfCollectibles::<T>::get(&sender);
            if let Some(index) = sender_collection.iter().position(|&element| element == collectible_id){
                sender_collection.swap_remove(index);
            } // Cannot be None if everything is well implemented, as we know this account owns the collectible due to the previous lines
            OwnerOfCollectibles::<T>::insert(sender, sender_collection);

            Self::deposit_event(Event::CollectibleDestroyed { collectible: collectible_id });
            Ok(())
        }

        /// Transfer a collectible to another account.
        /// Any account that holds a collectible can send it to another account. 
        /// Transfer resets the price of the collectible, marking it not for sale.
        #[pallet::weight(0)]
        pub fn transfer(
            origin: OriginFor<T>,
            to: T::AccountId,
            collectible_id: [u8; 16]
        ) -> DispatchResult {
            // Make sure the caller is from a signed origin
            let from = ensure_signed(origin)?;
            let collectible = CollectibleMap::<T>::get(&collectible_id).ok_or(Error::<T>::NoCollectible)?;
            ensure!(collectible.owner == from, Error::<T>::NotOwner);
            Self::do_transfer(collectible_id, to)?;
            Ok(())
        }

        #[pallet::weight(0)]
        pub fn set_price(
            origin: OriginFor<T>,
            collectible_id: [u8; 16],
            new_price: BalanceOf<T>
        ) -> DispatchResult{
            let from = ensure_signed(origin)?;
            let mut collectible = CollectibleMap::<T>::get(&collectible_id).ok_or(Error::<T>::NoCollectible)?;
            ensure!(collectible.owner == from, Error::<T>::NotOwner);
            collectible.price = Some(new_price);
            CollectibleMap::<T>::insert(collectible_id, collectible);
            Self::deposit_event(Event::PriceSet { collectible: collectible_id, price: new_price });
            Ok(())
        }

        #[pallet::weight(0)] // Same thoughts shared in destroy_collectible dispatchable
        pub fn remove_from_market(
            origin: OriginFor<T>,
            collectible_id: [u8; 16]
        ) -> DispatchResult{
            let from = ensure_signed(origin)?;
            let mut collectible = CollectibleMap::<T>::get(&collectible_id).ok_or(Error::<T>::NoCollectible)?;
            ensure!(collectible.owner == from, Error::<T>::NotOwner);
            ensure!(collectible.price.is_some(), Error::<T>::CollectibleNotForSale);
            collectible.price = None;
            CollectibleMap::<T>::insert(collectible_id, collectible);
            Self::deposit_event(Event::NotLongerOnSale { collectible: collectible_id });
            Ok(())
        }

        #[pallet::weight(0)] // Same thoughts shared in destroy_collectible dispatchable
        pub fn buy(
            origin: OriginFor<T>,
            collectible_id: [u8; 16],
            offered_price: BalanceOf<T>
        ) -> DispatchResult{
            let buyer = ensure_signed(origin)?; // Ensure that the buyer signed the transaction
            let collectible = CollectibleMap::<T>::get(&collectible_id).ok_or(Error::<T>::NoCollectible)?;
            ensure!(collectible.price.is_some(), Error::<T>::CollectibleNotForSale);
            ensure!(offered_price >= collectible.price.unwrap(), Error::<T>::OfferedPriceTooLow);
            Self::do_buy(collectible_id, buyer, offered_price)?;
            Ok(())
        }
    }
}
