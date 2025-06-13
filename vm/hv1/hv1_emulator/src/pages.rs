// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::VtlProtectAccess;
use guestmem::GuestMemory;
use guestmem::LockedPages;
use guestmem::Page;
use hvdef::HvMapGpaFlags;
use inspect::Inspect;
use safeatomic::AtomicSliceOps;
use std::ops::Deref;
use std::sync::atomic::AtomicU8;

pub(crate) struct LockedPage {
    page: LockedPages,
}

impl LockedPage {
    pub fn new(gpn: u64, guest_memory: &GuestMemory) -> Result<Self, guestmem::GuestMemoryError> {
        let page = match guest_memory.lock_gpns(false, &[gpn]) {
            Ok(it) => it,
            Err(err) => {
                tracelimit::error_ratelimited!(
                    gpn,
                    err = &err as &dyn std::error::Error,
                    "Failed to lock page"
                );
                return Err(err);
            }
        };
        assert!(page.pages().len() == 1);
        Ok(Self { page })
    }
}

impl Deref for LockedPage {
    type Target = Page;

    fn deref(&self) -> &Self::Target {
        self.page.pages()[0]
    }
}

#[derive(Inspect)]
#[inspect(external_tag)]
pub(crate) enum OverlayPage {
    Local(#[inspect(skip)] Box<Page>),
    Mapped {
        #[inspect(skip)]
        page: LockedPage,
        gpn: u64,
    },
}

// FUTURE: Technically we should restore the prior contents of a mapped location when we
// remap/unmap it, but we don't know of any scenario that actually requires this.
impl OverlayPage {
    pub fn remap(
        &mut self,
        new_gpn: u64,
        guest_memory: &GuestMemory,
        prot_access: &mut dyn VtlProtectAccess,
    ) -> Result<(), hvdef::HvError> {
        prot_access.check_modify_and_lock_overlay_page(
            new_gpn,
            HvMapGpaFlags::new().with_readable(true).with_writable(true),
            None,
        )?;
        let new_page = LockedPage::new(new_gpn, guest_memory).unwrap();
        new_page.atomic_write_obj(&self.atomic_read_obj::<[u8; 4096]>());

        self.unlock_prev_gpn(prot_access);

        *self = OverlayPage::Mapped {
            page: new_page,
            gpn: new_gpn,
        };
        Ok(())
    }

    pub fn unmap(&mut self, prot_access: &mut dyn VtlProtectAccess) {
        let new_page = Box::new(std::array::from_fn(|_| AtomicU8::new(0)));
        new_page.atomic_write_obj(&self.atomic_read_obj::<[u8; 4096]>());

        self.unlock_prev_gpn(prot_access);

        *self = OverlayPage::Local(new_page);
    }

    fn unlock_prev_gpn(&mut self, prot_access: &mut dyn VtlProtectAccess) {
        if let Self::Mapped { gpn: prev_gpn, .. } = self {
            prot_access.unlock_overlay_page(*prev_gpn).unwrap();
        }
    }
}

impl Deref for OverlayPage {
    type Target = Page;

    fn deref(&self) -> &Self::Target {
        match self {
            OverlayPage::Local(page) => page,
            OverlayPage::Mapped { page, .. } => page,
        }
    }
}

impl Default for OverlayPage {
    fn default() -> Self {
        OverlayPage::Local(Box::new(std::array::from_fn(|_| AtomicU8::new(0))))
    }
}
