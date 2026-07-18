# PSTForge Writer Conformance Index

This document is the specification ledger for every byte and relationship that
`pstforge-pst` deliberately writes. It is an audit index, not a claim that the
audit is complete. A writer behavior is eligible for implementation or change
only after its entry identifies a normative Microsoft source, the relevant code,
a focused test, and independent evidence.

The audit is non-destructive. `Empirical` and `Unresolved` output remains in the
writer while it is investigated. PSTForge will not remove, disable, or narrow
such output until the human owner has reviewed the available requirements,
interoperability evidence, preservation impact, and concrete options.

## Status

- `Verified`: implementation and focused test have been compared with the
  cited normative requirement.
- `Partial`: part of the grouped invariant is verified; the notes identify the
  remaining comparison.
- `Pending`: the implementation exists but the source-to-code comparison has
  not been completed.
- `Empirical`: Microsoft does not document the emitted value or structure in
  the located specifications. The behavior is retained pending human decision.
- `Conflict`: implementation contradicts a located normative requirement. A
  focused correction and acceptance checkpoint is required.
- `Accepted interoperability exception`: strict generic specification wording
  produced demonstrably corrupt user-visible output, while the retained
  behavior passed independent integrity and product checks. The exception is
  frozen by a focused regression and human decision.
- `Accepted recovery policy`: Microsoft defines the required structure but not
  the recovery value. The human owner has accepted the documented behavior and
  its accounting contract.
- `Pending owner decision`: the current behavior is preserved and explicitly
  accounted for, but alternatives still require comparative evidence and a
  human choice.

Reader tolerance is supplemental evidence. PSTForge reading its own output,
`libpff`, `pffinfo`, `readpst`, ScanPST, Outlook, and MailPlus do not replace a
normative source.

## Source Baseline

The baseline is the current Microsoft Learn online revision accessed
2026-07-18. The page date below is the page's published `Last updated` value.
Before changing a verified behavior, recheck the live page and record a changed
date or protocol revision here.

### PST-NDB
- **Source:** [MS-PST NDB Layer](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/9d2083cf-fd37-4a0d-b61a-d2ef10a89a04)
- **Section:** 2.6.1
- **Page date:** 2022-11-15

### PST-LTP
- **Source:** [MS-PST LTP Layer](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/77007716-7993-44fe-9b40-9526157cfc6d)
- **Section:** 2.3
- **Page date:** 2019-02-14

### PST-MV-FIXED
- **Source:** [MS-PST MV Properties with Fixed-size Base Type](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/73b910ea-09c0-4512-8cd2-e98d06497d51)
- **Section:** 2.3.3.4.1
- **Page date:** 2024-04-16

### PST-MSG
- **Source:** [MS-PST Messaging Layer](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/e2e7a5aa-c89f-4fb8-b044-15ac76e5207e)
- **Section:** 2.4
- **Page date:** 2024-11-12

### PST-INT
- **Source:** [MS-PST Maintaining Data Integrity](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/5e1a4d6b-ebbf-4658-9aa7-824929233044)
- **Section:** 2.6
- **Page date:** 2021-02-16

### PST-PB
- **Source:** [MS-PST Product Behavior](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/f040f8b2-f023-4ed9-94fd-de487da83ed5)
- **Section:** Appendix B
- **Page date:** 2021-08-17

### PST-NAMEID
- **Source:** [MS-PST Named Property Lookup Map](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/e17e195d-0454-4b9b-b398-c9127a26a678)
- **Section:** 2.4.7
- **Page date:** 2022-11-15

### PST-PROPS
- **Source:** [MS-PST Properties](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/36c1290e-8b1b-4d8c-91e1-d9fb3147c11c)
- **Section:** 2.5
- **Page date:** 2022-11-15

### PST-STORE
- **Source:** [MS-PST Minimum Store Properties](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/5493a0eb-0356-4e88-b4f5-0433ce0a93fa)
- **Section:** 2.4.3.1
- **Page date:** 2020-10-15

### PST-EID
- **Source:** [MS-PST EntryID and NID](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/9378e8b9-7b6a-45bf-a51a-f21daf24d9ce)
- **Section:** 2.4.3.2
- **Page date:** 2024-11-19

### PST-MESSAGE
- **Source:** [MS-PST Message Objects](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/1042af37-aaa4-4edc-bffd-90a1ede24188)
- **Section:** 2.4.5
- **Page date:** 2022-11-15

### PST-ATTACH
- **Source:** [MS-PST Attachment Objects](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/46eb4828-c6a5-420d-a137-9ee36df317c1)
- **Section:** 2.4.6
- **Page date:** 2021-10-05

### PST-CONTENTS
- **Source:** [MS-PST Contents Table Template](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/f58e1ea9-b592-408d-b89e-53fd4cd6024b)
- **Section:** 2.4.4.5.1
- **Page date:** 2019-02-14

### PST-MANDATORY
- **Source:** [MS-PST Mandatory Nodes](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/661f9921-54ff-4768-b98c-91954312af52)
- **Section:** 2.7.1
- **Page date:** 2020-10-15

### PST-TEMPLATES
- **Source:** [MS-PST Template Objects](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/c1af6316-b8a4-4b17-883e-3a60189f361c)
- **Section:** 2.7.3.3
- **Page date:** 2019-02-14

### PST-HIERARCHY
- **Source:** [MS-PST Hierarchy Table Template](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/c08fb6cb-2d91-42e5-b70d-f3e4f9781a2a)
- **Section:** 2.4.4.4.1
- **Page date:** 2021-08-17

### PST-FAI
- **Source:** [MS-PST FAI Contents Table Template](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/b2e619a0-6a9c-4101-9dcb-340ac41cf308)
- **Section:** 2.4.4.6.1
- **Page date:** 2025-02-18

### PST-SEARCH-TC
- **Source:** [MS-PST Search Folder Contents Table Template](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/cdcf9571-049f-47f5-b075-8374057134ec)
- **Section:** 2.4.8.6.2.1
- **Page date:** 2020-10-15

### PST-RECIP-TC
- **Source:** [MS-PST Recipient Table Template](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/bb069b2b-80ad-46d5-b86f-33487d16bf0c)
- **Section:** 2.4.5.3.1
- **Page date:** 2022-11-15

### PST-ATTACH-TC
- **Source:** [MS-PST Attachment Table Template](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/47c336f7-2d9b-4f22-91c7-5bb422aaebbb)
- **Section:** 2.4.6.1.1
- **Page date:** 2021-08-17

### PST-FOLDER-PC
- **Source:** [MS-PST Folder PC Schema](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/ec5b8b40-8b31-4612-88c8-510745f7ae80)
- **Section:** 2.4.4.1.1
- **Page date:** 2020-10-15

### PST-MESSAGE-PC
- **Source:** [MS-PST Message PC Schema](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/6a967f44-cec3-403d-9100-7313656cc65c)
- **Section:** 2.4.5.1.1
- **Page date:** 2022-11-15

### PST-ATTACH-PC
- **Source:** [MS-PST Attachment PC Schema](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/37b3a8d1-acde-4759-820d-6febd7befba8)
- **Section:** 2.4.6.2.1
- **Page date:** 2021-08-17

### PST-NID
- **Source:** [MS-PST NID](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/18d7644e-cb33-4e11-95c0-34d8a84fbff6)
- **Section:** 2.2.2.1
- **Page date:** 2020-10-15

### PST-AMAP
- **Source:** [MS-PST AMap Page](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/60466ef4-af15-49b6-8413-b3a72f0e9bdb)
- **Section:** 2.2.2.7.2
- **Page date:** 2024-04-16

### PST-PMAP
- **Source:** [MS-PST PMap Page](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/e0c59db8-970a-40df-9547-c136e8858291)
- **Section:** 2.2.2.7.3
- **Page date:** 2019-02-14

### PST-FPMAP
- **Source:** [MS-PST FPMap Page](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/dd913b8e-5113-4b83-a5ea-351a08b4237b)
- **Section:** 2.2.2.7.6
- **Page date:** 2019-02-14

### PST-PAGE
- **Source:** [MS-PST PAGETRAILER](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/f4ccb38a-930a-4db4-98df-a69c195926ba)
- **Section:** 2.2.2.7.1
- **Page date:** 2022-11-15

### PST-BLOCK
- **Source:** [MS-PST Blocks](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/a9c1981d-d1ea-457c-b39e-dc7fb0eb95d4)
- **Section:** 2.2.2.8
- **Page date:** 2019-02-14

### PST-ROW
- **Source:** [MS-PST Row Data Format](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/c48fa6b4-bfd4-49d7-80f8-8718bc4bcddc)
- **Section:** 2.3.4.4
- **Page date:** 2022-11-15

### PST-TCINFO
- **Source:** [MS-PST TCINFO](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/45b3a0c5-d6d6-4e02-aebf-13766ff693f0)
- **Section:** 2.3.4.1
- **Page date:** 2022-11-15

### OXCDATA-TYPES
- **Source:** [MS-OXCDATA Property Data Types](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcdata/0c77892e-288e-435a-9c49-be1c20c7afdb)
- **Section:** 2.11.1
- **Page date:** 2025-05-20 protocol revision

### OXPROPS-CONTAINER
- **Source:** [MS-OXPROPS PidTagContainerClass](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxprops/ce088491-2739-46c0-a20a-2e9adc7d3856)
- **Section:** 2.643
- **Page date:** 2025-05-20

### MAPI-RECEIVE
- **Source:** [Receive Folder Tables](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/receive-folder-tables)
- **Section:** required column set
- **Page date:** 2022-03-03

### MAPI-OUTGOING
- **Source:** [Outgoing Queue Tables](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/outgoing-queue-tables)
- **Section:** required column set
- **Page date:** 2022-03-23

### MAPI-FAI
- **Source:** [Folder-Associated Information Tables](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/folder-associated-information-tables)
- **Section:** FAI table contract
- **Page date:** 2022-01-22

### MAPI-FLAGS
- **Source:** [PidTagMessageFlags](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/pidtagmessageflags-canonical-property)
- **Section:** `MSGFLAG_ASSOCIATED`
- **Page date:** 2022-03-17

### MAPI-CONTENTS
- **Source:** [Contents Tables](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/contents-tables)
- **Section:** normal/associated contents
- **Page date:** 2022-01-22

### MAPI-FOLDERS
- **Source:** [MAPI Folders](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/mapi-folders)
- **Section:** hierarchy and special folders
- **Page date:** 2022-01-22

### MAPI-CONTAINER
- **Source:** [PidTagContainerClass](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/pidtagcontainerclass-canonical-property)
- **Section:** folder class values
- **Page date:** 2022-05-27

### OXOMSG
- **Source:** [MS-OXOMSG Email Object Protocol](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxomsg/daa9120f-f325-4afb-a738-28f91049ab3c)
- **Section:** published revision 24.0
- **Page date:** 2025-05-20

### OXOCNTC
- **Source:** [MS-OXOCNTC Contact Object Protocol](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocntc/9b636532-9150-4836-9635-9c9b756c9ccf)
- **Section:** published revision 21.0; [contacts-related folders 2.2.3](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocntc/45951a5e-82b4-4d83-83e6-24abbac67947)
- **Page date:** 2025-05-20

### OXOCAL
- **Source:** [MS-OXOCAL Appointment and Meeting Object Protocol](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocal/09861fde-c8e4-4028-9346-e7c214cfdba1)
- **Section:** published revision 22.1
- **Page date:** 2025-08-19

### OXOTASK
- **Source:** [MS-OXOTASK Task-Related Objects Protocol](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxotask/55600ec0-6195-4730-8436-59c7931ef27e)
- **Section:** published revision 16.0
- **Page date:** 2025-05-20

### OXONOTE
- **Source:** [MS-OXONOTE Note Object Protocol](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxonote/6bf4ed7e-316c-4a3c-be27-5ec93e7ab39f)
- **Section:** published revision 12.0
- **Page date:** 2025-05-20

### OXOPOST
- **Source:** [MS-OXOPOST Post Object Protocol](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxopost/9b18fdab-aacd-4d73-9534-be9b6ba2f115)
- **Section:** published revision 13.0
- **Page date:** 2025-05-20

### OXOCFG
- **Source:** [MS-OXOCFG Configuration Information Protocol](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocfg/7d466dd5-c156-4da9-9a01-75c78e7e1a67); [FAI relationship](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocfg/ae9bb189-eb03-4e58-b05f-8f348f1768b5)
- **Section:** published revision 16.0; section 1.4
- **Page date:** 2025-05-20

The audit will add the exact MS-PST child-page URL and page date when a grouped
overview reference is replaced by a section-specific reference.

## NDB And Physical Layout

### NDB-01
- **Status:** Verified: field order/width, constants, zeroed reserved fields, FM/FP bytes, root references/free counts, creation counters, BID/NID type rules, and both CRC ranges match. Exact new-store seed values not prescribed by Microsoft are isolated in EMP-10.
- **Requirement:** Unicode version-23 header fields, roots, crypt method, NID/BID high-water marks, and header CRCs
- **Sources:** [MS-PST 2.2.2.6 HEADER](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/c9876f5a-664b-46a3-9887-ba63f113abf5); [MS-PST 2.2.2.5 ROOT](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/32ce8c94-4757-46c8-a169-3fd21abee584); PST-PB notes 5-13
- **Implementation:** `write_header`, `UnicodeHeader::new_store`, `nid_counters`, `crc`
- **Evidence:** `new_store_round_trips_through_upstream_reader`, `header_crc_rejects_tampering`, `scanpst_required_metadata_is_serialized`; ScanPST r2

### NDB-02
- **Status:** Verified: 64-byte blocks, 512-byte pages, Unicode trailer widths/order, logical byte counts, padding placement, BID internal bit, CRC input, signature input, and external-data permutation agree.
- **Requirement:** Page and block alignment, trailers, signatures, CRCs, BID parity, and payload encoding
- **Sources:** [MS-PST 2.2.2.7.1 PAGETRAILER](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/f4ccb38a-930a-4db4-98df-a69c195926ba); [MS-PST 2.2.2.8 Blocks](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/a9c1981d-d1ea-457c-b39e-dc7fb0eb95d4); [MS-PST 2.2.2.8.1 BLOCKTRAILER](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/a14943ef-70c2-403f-898c-5bc3747117e1); MS-PST 2.2.2.2-2.2.2.3 and 5.1, 5.3, 5.5
- **Implementation:** `write_blocks`, adapted `BlockReadWrite`/`IntermediateTreeBlockReadWrite`, `page_trailer`, `leaf_bid`, `internal_bid`, `compute_sig`, `permute`, `crc`
- **Evidence:** upstream-reader roundtrip; CRC-tampering tests; `pffinfo`; ScanPST r2

### NDB-03
- **Status:** Verified: nonempty leaf/XBLOCK/XXBLOCK levels, 8,176-byte payloads, 1,021-entry limits, `lcbTotal`, ordering, and streamed hashing match. Empty binary/Unicode values use the inline empty-value path; a zero-byte spool descriptor is rejected in preflight and can no longer create the prohibited zero-length NDB block.
- **Requirement:** Data trees and extended blocks represent bounded and streamed values without truncation
- **Sources:** [MS-PST 2.2.2.8.3.1 Data Blocks](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/d0e6fbaf-00e3-4d4d-bea8-8ab3cdb4fde6); [MS-PST 2.2.2.8.3.2.1 XBLOCK](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/5b7a6935-e83d-4917-9f62-6ce3707f09e0); [MS-PST 2.2.2.8.3.2.2 XXBLOCK](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/061b6ac4-d1da-468c-b75d-0303a0a8f468)
- **Implementation:** `append_data_tree*`, `append_spooled_data_tree`, `externalize_large_properties`, `validate_message`
- **Evidence:** `spooled_attachment_streams_across_data_tree_groups`, empty-value/preflight and boundary tests; `pffinfo`

### NDB-04
- **Status:** Verified: sorted local NIDs, entry widths, data/subnode BIDs, nonempty SLBLOCKs, level-0 leaves, and the single permitted level-1 SIBLOCK agree. The exact 340-by-510 capacity is checked before mutation, so 173,401 entries return a bounded error instead of emitting an invalid level-2 SIBLOCK.
- **Requirement:** Subnode B-trees preserve local node identity and embedded/attachment relationships
- **Sources:** [MS-PST 2.2.2.8.3.3.1.1 SLENTRY](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/85c4d943-0779-43c5-bd98-61dc9bb5dfd6); [2.2.2.8.3.3.1.2 SLBLOCK](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/5182eb24-4b0b-4816-aa3f-719cc6e6b018); [2.2.2.8.3.3.2.1 SIENTRY](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/9e79c673-d2f4-49fb-a00b-51b08fd2d1e4); [2.2.2.8.3.3.2.2 SIBLOCK](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/729fb9bd-060a-4bbc-9b3b-8f014b487dad)
- **Implementation:** `append_subnode_tree`, `node_entries`, `build_message_blocks`
- **Evidence:** rich-mail and embedded-message roundtrips; exact maximum-depth guard; ScanPST fidelity candidates

### NDB-05
- **Status:** Verified: Unicode leaf/intermediate entry widths, 20-entry BBT/intermediate capacity, 15-entry NBT-leaf capacity, first-key parent separators, sorted keys, page levels, page BIDs, root BREFs, and balanced non-root occupancy match. Constructors enforce the documented maximum depth.
- **Requirement:** NBT and BBT leaf/intermediate pages are sorted, bounded, linked, and rooted correctly
- **Sources:** MS-PST 2.2.2.7.7 and child structure pages
- **Implementation:** `write_bbt`, `write_nbt`, `plan_leaf_pages`
- **Evidence:** `btree_leaf_planning_splits_at_ms_pst_capacity`; ScanPST 19 GB parts

### NDB-06
- **Status:** Verified for the DList allocation mode used by supported Outlook generations: first offsets, recurring intervals, reserved pages, AMap self-allocation, extent bits/free counts, page conventions/checksums, and large-file rebuild match. Deprecated PMap/FMap/FPMap pages are retained at required intervals and are not used for allocation, consistent with MS-PST product behavior.
- **Requirement:** AMap/PMap/FMap/FPMap/DList pages occur at required intervals and allocation bits cover exactly written extents
- **Sources:** [MS-PST 2.2.2.7.2 AMap](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/60466ef4-af15-49b6-8413-b3a72f0e9bdb); [2.2.2.7.3 PMap](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/e0c59db8-970a-40df-9547-c136e8858291); MS-PST 2.2.2.7.4-2.2.2.7.5; [2.2.2.7.6 FPMap](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/dd913b8e-5113-4b83-a5ea-351a08b4237b); PST-PB notes 14-17
- **Implementation:** `write_fixed_pages`, `reserved_map_page_count`, `allocate_extent`, `allocation_file_eof`, adapted allocation-map rebuild
- **Evidence:** allocation-map and FPMap-boundary tests; ScanPST 4 GB parts

### NDB-07
- **Status:** Verified: PSTForge builds a new private file rather than modifying a published PST, syncs it, completes any allocation-map rebuild and resync, validates all owned relationships, requires `pffinfo` and `readpst`, atomically renames without replacement, syncs the held destination directory, and verifies the published device/inode. Failure before rename leaves no public part; post-rename durability uncertainty is reported distinctly.
- **Requirement:** File publication occurs only after internal and independent validation, file `fsync`, atomic no-clobber rename, and directory `fsync`
- **Sources:** PST-INT 2.6; the verified NDB/LTP/Messaging rows above; POSIX durability is a PSTForge safety requirement
- **Implementation:** `create_flat_store`, `validate_completed_store`, `validate_completed_folder_store`, `validate_with_independent_readers`, `publish_noclobber`, `sync_published_directory`, `verify_published_destination`
- **Evidence:** publication, timeout, retained-candidate, moved-directory, no-clobber, and validator-scratch tests

## LTP Structures

### LTP-01
- **Status:** Verified: signatures, client types, root HID, page-header cadence, 2-byte map alignment, allocation/free counts, offset endpoints, 3,580-byte allocation maximum, and root/bitmap fill-level ranges agree.
- **Requirement:** Heap-on-node headers, allocation maps, page maps, fill levels, HIDs, and continuation pages are structurally valid
- **Sources:** [MS-PST 2.3.1.2 HNHDR](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/8e4ae05c-3c24-4103-b7e5-ffef6f244834), 2.3.1.3-2.3.1.4 continuation headers, [2.3.1.5 HNPAGEMAP](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/291653c0-b347-4c5b-ba41-85ad780b4ba4), and 2.3.1.6; [2.6.2.1.2 allocation](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/5b30032e-8cbc-4f03-a6bd-c21a7f1c54ea)
- **Implementation:** `heap_page`, `heap_continuation_page`, `fill_heap_page`, `update_heap_fill_levels`
- **Evidence:** `property_context_heap_round_trips`, `external_table_fills_every_non_final_heap_page`; ScanPST

### LTP-02
- **Status:** Verified: `bTypeBTH`, permitted key/value widths, zero/positive roots, index-level count, sorted first-key separators, PC 2/6 records, and TC row-index 4/4 records agree.
- **Requirement:** BTH headers and records use documented key/value widths and sorted keys
- **Sources:** [MS-PST 2.3.2.1 BTHHEADER](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/5a6ab19e-1f44-4def-ad64-7bd82d94bd78), 2.3.2.2 intermediate records, and 2.3.2.3 leaf records
- **Implementation:** `property_context`, `table_context*`, LTP `tree`
- **Evidence:** property-context and rich-mail roundtrips; `pffinfo`

### LTP-03
- **Status:** Verified: property tags, <=4-byte inline values, heap HIDs, >2,048-byte PSTForge subnodes, object NID/size pairs, supported scalar byte order, and packed fixed-width multivalues agree. Exact-length Unicode remains the accepted EMP-11 interoperability exception.
- **Requirement:** Property contexts encode each supported property type using the documented inline, heap, or subnode representation
- **Sources:** [MS-PST 2.3.3 PC](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/294c83c6-ff92-42f5-b6b6-876c29fa9737), [2.3.3.3 PC BTH record](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/7daab6f5-ce65-437e-80d5-1b1be4088bd3), [2.3.3.5 PtypObject](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/49457d57-820e-453d-bbc0-1d192a999814), PST-PROPS, and OXCDATA-TYPES
- **Implementation:** `property_context`, `externalize_large_properties`, `raw_property_value`
- **Evidence:** `every_supported_raw_value_round_trips`, external-property boundary tests

### LTP-04
- **Status:** Verified: RowID/RowVer offsets and bits, 4/2/1-byte regions, HNID column widths, TCINFO boundaries, sorted row BTH, tight row matrix, MSB-first CEB, zero unused bits, and heap/subnode variable values agree.
- **Requirement:** Table-context column descriptors, row index, existence bitmap, row matrix, and external values agree
- **Sources:** PST-TCINFO; [MS-PST 2.3.4.2 TCOLDESC](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/3a2f63cf-bb40-4559-910c-e55ec43d9cbb); [2.3.4.4.1 Row Data](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/c48fa6b4-bfd4-49d7-80f8-8718bc4bcddc); [2.3.4.4.2 variable data](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/a8da3d66-6051-4e30-8b8c-2b7d3c373834)
- **Implementation:** `table_context*`, `schema_columns`, `write_table_value`, `mark_column`
- **Evidence:** multi-page contents and rich-mail tests; ScanPST table validation

## Messaging Objects And Tables

### MSG-01
- **Status:** Verified for the MS-PST minimum graph and store-PC properties; non-mandatory nodes are tracked below
- **Requirement:** Every store contains the mandatory nodes, store PC, record key, IPM subtree, wastebasket, finder, and EntryIDs
- **Sources:** PST-MANDATORY; PST-STORE; PST-EID
- **Implementation:** `node_entries`, `store_properties`, `entry_id`
- **Evidence:** `scanpst_required_metadata_is_serialized`, upstream-reader roundtrip; ScanPST r2

### MSG-02
- **Status:** Partial: ordinary source folders match the required PC/three-TC structure; fixed-root count discrepancy is EMP-07
- **Requirement:** Folder PCs and hierarchy/contents/associated table nodes agree on parentage, counts, unread state, and child rows
- **Sources:** PST-FOLDER-PC; MS-PST 2.4.4; MAPI-FOLDERS; MAPI-CONTENTS
- **Implementation:** `plan_folders`, `folder_properties_with_unread`, `folder_table_row_with_unread`, `node_entries`
- **Evidence:** nested/root folder tests; Outlook 19 GB parts

### MSG-03
- **Status:** Verified for required Message-PC fields and recipient containment; optional empty attachment-table output is retained pending procedural audit
- **Requirement:** Normal message PC, recipient table, optional attachment table, and attachment subnodes are contained under one top-level message node
- **Sources:** PST-MESSAGE; PST-MESSAGE-PC
- **Implementation:** `build_message_blocks`, `message_properties`, `recipient_table_row`, `attachment_table_row`
- **Evidence:** rich-mail and embedded roundtrips; ScanPST fidelity candidates

### MSG-04
- **Status:** Partial: complete required ID set and row/PC equality verified; MS-PST type contradiction is EMP-06
- **Requirement:** Contents-table rows use the mandatory template columns and match message PCs
- **Sources:** PST-CONTENTS; MS-PST 2.4.4.3
- **Implementation:** `contents_columns`, `message_table_row`, `set_message_size`
- **Evidence:** multi-message/index tests; ScanPST 19 GB parts

### MSG-05
- **Status:** Verified for the required template column ID/type set; optional recipient values remain class-specific
- **Requirement:** Recipient table is always present, has required columns, and preserves recipient type and address properties
- **Sources:** PST-RECIP-TC; PST-MESSAGE 2.4.5.3
- **Implementation:** `recipient_columns`, `recipient_table_row`, `display_recipient_properties`
- **Evidence:** rich-mail roundtrip; Outlook fidelity acceptance

### MSG-06
- **Status:** Verified for the required template/PC fields and table/subnode cardinality
- **Requirement:** Attachment table rows and attachment PCs share attachment number/method/size and preserve by-value data
- **Sources:** PST-ATTACH-TC; PST-ATTACH-PC; PST-ATTACH 2.4.6.1-2.4.6.3
- **Implementation:** `attachment_columns`, `attachment_table_row`, `attachment_properties`, `set_attachment_size`
- **Evidence:** huge/spooled/rich attachment tests; Outlook and MailPlus acceptance

### MSG-07
- **Status:** Verified: method 5, attachment-owned message subnode, object NID/size pair, child PC, recipient table, attachment table, and recursive child subnodes agree. The 256-level traversal bound is a product containment limit, not a PST format limit.
- **Requirement:** Embedded messages use `ATTACH_EMBEDDED_MSG`, the documented attachment subnode relationship, their own recipient/attachment subnodes, and bounded recursive traversal
- **Sources:** PST-ATTACH 2.4.6.3; [MS-PST 2.3.3.5 PtypObject](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/49457d57-820e-453d-bbc0-1d192a999814); PST-MESSAGE
- **Implementation:** `build_message_blocks`, `embedded_message_record_key`, `validate_embedded_message`
- **Evidence:** embedded named-property and RTF tests; Outlook acceptance

### MSG-08
- **Status:** Verified
- **Requirement:** Associated messages are written only to the associated contents table and carry `MSGFLAG_ASSOCIATED` in both PC and table row; normal/embedded messages do not
- **Sources:** MAPI-FAI; MAPI-FLAGS; MAPI-CONTENTS
- **Implementation:** `output_message_flags`, `associated_message_table_row`, `build_message_blocks`
- **Evidence:** `root_folders_and_associated_messages_keep_their_source_placement`; ScanPST/Outlook r2

### MSG-09
- **Status:** Verified for populated mappings: store-wide identity collection, deterministic property indices, reserved/custom GUID selectors, numeric/string entry forms, UTF-16 byte lengths and padding, LID/CRC hashing, 251 buckets, and bucket contents match. The reserved-only/empty GUID and entry sentinel remains isolated in EMP-03.
- **Requirement:** Named-property streams map numeric/string names and reserved/custom GUID selectors deterministically across top-level and embedded messages
- **Sources:** PST-NAMEID 2.4.7, including [entry stream](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/e17e195d-0454-4b9b-b398-c9127a26a678), [string stream](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/bbf3cbf6-74f4-48f0-899d-7d79650c021f), [GUID stream](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/0f67b30c-0891-44ef-9a80-24d43ba1b28c), and [hash buckets](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/6d390cac-0a02-4a34-9a93-e04e26f149ee)
- **Implementation:** `collect_named_identities*`, `named_property_map`, `validate_named_map`
- **Evidence:** named ordering, custom GUID, empty map, and embedded map tests; ScanPST fidelity candidates

### MSG-10
- **Status:** Verified: plain Unicode body, binary HTML, valid literal-only LZFu container with header/end marker/CRC, RTF synchronization flag, native-body enumeration, and the recovered or product-default Internet code page use the documented IDs and types. Absent body representations are not fabricated. The generated code-page fallback is EMP-14.
- **Requirement:** Message bodies preserve plain text, HTML, compressed RTF, and RTF synchronization properties without synthesizing absent bodies
- **Sources:** [MS-OXCMSG PidTagNativeBody](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcmsg/71428f1c-a004-4c05-bc8e-6a687de06a2e); [MS-OXCMSG PidTagHtml](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcmsg/bf35ff72-9a42-428c-b376-8a8928b821dc); [MS-OXRTFCP 2.1.3.1 compression header](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxrtfcp/4c589b4d-6334-418e-93fd-1c75f820e770); [PidTagRtfInSync](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/pidtagrtfinsync-canonical-property); [PidTagInternetCodepage](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/pidtaginternetcodepage-canonical-property)
- **Implementation:** `message_properties`, `rtf_container`, `rtf_container_len`
- **Evidence:** compressed RTF and native-body tests; Outlook/MailPlus fidelity acceptance

### MSG-11
- **Status:** Partial: the generic scalar/named/raw-property preservation path is verified, and completed class families are enumerated below. Unimplemented 0.4.2 class families remain pending their own exact protocol pass. Generated missing-source metadata is isolated in EMP-14.
- **Requirement:** Arbitrary readable message classes and raw properties retain their property type/value unless a documented generated property owns the tag
- **Sources:** PST-PROPS; OXCDATA-TYPES; class-specific MS-OX* documents
- **Implementation:** `supported_message_class`, `explicit_message_property`, `raw_property_value`
- **Evidence:** all-raw-value and class checkpoint tests; ScanPST/Outlook checkpoints

### MSG-12
- **Status:** Partial: exact embedded class, method 5 object, start `0x7FFB`, end `0x7FFC`, flags `0x7FFD`, hidden `0x7FFE`, display name, encoding, rendering data, and embedded content agree. The accepted source-derived `0x7FFA`/`0x7FFF` values and the absence of documented replacement-time `0x7FF9` are isolated in EMP-13 and remain unchanged.
- **Requirement:** Calendar exception attachment properties retain documented linkage and embedded exception content
- **Sources:** [MS-OXOCAL 2.2.10 Exceptions](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocal/ad438f25-c933-44af-afbb-bb20bc876a0b); [2.2.10.1.1 PidTagAttachmentHidden](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocal/4968bd1c-eeed-4f32-8d0f-e732cee09b5d); [2.2.10.1.6 PidTagExceptionReplaceTime](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocal/22956d67-d5cb-4db2-aa49-a6f15d24de7a); [MS-OXOCAL creation example](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocal/7d95cc80-48b7-4ad2-93fb-767b6962ff8c); [MS-OXCICAL RECURRENCE-ID](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcical/6911f0f9-a26b-44bd-be7e-0fe38059fae0)
- **Implementation:** `calendar_exception_attachment_*`, `validate_attachment_fidelity`
- **Evidence:** `calendar_exception_attachment_properties_round_trip`; exact libpff source/output fingerprint; ScanPST/Outlook accepted checkpoint

## Completed Message-Class Preservation

These entries verify that completed 0.4.2 checkpoints preserve the source class,
folder class, and every representable source property. They do not authorize
PSTForge to invent class semantics that were absent from the source.

### CLS-01
- **Status:** Verified for readable properties supported by the typed writer; missing-source display fallbacks are EMP-14
- **Class family:** `IPM.Note`, descendants, and report descendants
- **Sources:** OXOMSG; MS-OXCMSG
- **Behavior and evidence:** Ordinary mail routing, body, attachment, raw, and named properties use the generic message model; exact external fingerprints and MailPlus/Outlook acceptance cover the implemented subset

### CLS-02
- **Status:** Verified for Contact objects; Personal Distribution Lists remain a later checkpoint
- **Class family:** `IPM.Contact` and descendants
- **Sources:** OXOCNTC; MAPI-CONTAINER
- **Behavior and evidence:** Sender is optional, source `IPF.Contact` is retained, and contact raw/named properties remain on their owning message; exact libpff source/output fingerprint, ScanPST, and Outlook pass

### CLS-03
- **Status:** Verified for standalone appointments; recurrence exception conflict is MSG-12/EMP-13
- **Class family:** `IPM.Appointment` and descendants
- **Sources:** OXOCAL; MAPI-CONTAINER
- **Behavior and evidence:** Sender is optional, source `IPF.Appointment` is retained, and appointment raw/named properties are not derived from subject/body/timestamps; exact libpff source/output fingerprint, ScanPST, and Outlook pass

### CLS-04
- **Status:** Verified for request/response/cancellation families admitted by checkpoint 5
- **Class family:** `IPM.Schedule.Meeting.*`
- **Sources:** OXOCAL; MS-OXCMSG
- **Behavior and evidence:** Mail sender/recipient rules remain active and exact meeting/appointment named properties retain their source owner, type, and bytes; exact libpff source/output fingerprint, ScanPST, and Outlook pass

### CLS-05
- **Status:** Verified for standalone tasks; task communications remain a later checkpoint
- **Class family:** standalone `IPM.Task`
- **Sources:** OXOTASK; MAPI-CONTAINER
- **Behavior and evidence:** Sender is optional, source `IPF.Task` and task named properties are retained; exact libpff source/output fingerprint, ScanPST, and Outlook pass

### CLS-06
- **Status:** Verified
- **Class family:** `IPM.StickyNote`
- **Sources:** OXONOTE; MAPI-CONTAINER
- **Behavior and evidence:** Sender is optional, source `IPF.StickyNote`, body, and note named properties are retained; exact libpff source/output fingerprint, ScanPST, and Outlook pass

### CLS-07
- **Status:** Verified
- **Class family:** `IPM.Post`
- **Sources:** OXOPOST; MAPI-CONTAINER
- **Behavior and evidence:** Normal sender handling, source `IPF.Note`, body, and post properties are retained; exact libpff source/output fingerprint, ScanPST, and Outlook pass

### CLS-08
- **Status:** Verified for generic FAI preservation; interpreting configuration streams is neither required nor performed
- **Class family:** folder-associated configuration messages
- **Sources:** OXOCFG section 1.4; MAPI-FAI; MSG-08
- **Behavior and evidence:** Source traversal, not class-name inference, places the item in the owning folder's FAI table; class and representable raw/named properties remain exact

## Template And Outlook-Maintained Output

These entries are deliberately separate because reader acceptance cannot
establish their format. No entry authorizes removing the existing output.

### EMP-01
- **Status:** Empirical. Retain unchanged. After the audit, present retain/rebuild/omit options and preservation impact to the owner.
- **Requirement:** The hierarchy map node `0xC01` contains a fixed 124-byte HMP payload copied from deterministic ScanPST output
- **Sources:** MS-PST does not define the HMP payload; ScanPST identifies HMP as an Outlook-maintained structure
- **Implementation:** `hierarchy_map`, `NID_HIERARCHY_MAP`, `node_entries`
- **Evidence:** ScanPST accepts current candidates; historical omission caused HMP repair findings

### EMP-02
- **Status:** Partial/Empirical. Retain unchanged while separating documented mandatory-node requirements from undocumented template bytes.
- **Requirement:** Contents/search/attachment index template nodes use fixed IDs, schemas, and a persisted-view template node whose NID has reserved type bits
- **Sources:** MS-PST 2.7.1 and table-template sections document mandatory nodes; exact `0x6B6` persisted-view payload/type origin not yet located
- **Implementation:** `NID_*_INDEX_TEMPLATE`, `contents_index_columns`, `search_index_columns`, `attachment_index_columns`, `update_nid_counter`
- **Evidence:** `scanpst_required_metadata_is_serialized`; ScanPST accepts current candidates

### EMP-03
- **Status:** Empirical. Retain both sentinels unchanged pending Outlook-created empty/reserved-only controls and human disposition.
- **Requirement:** An empty NAMEID map includes a fixed reserved MAPI mapping and hash bucket, and a populated map with only reserved GUID sets still includes a physical 16-byte MAPI GUID stream
- **Sources:** PST-NAMEID documents the five map properties and reserved GUID selectors, but custom GUID stream entries are indexed starting at selector 3; it does not require either sentinel
- **Implementation:** `named_property_map` empty and no-custom-GUID branches
- **Evidence:** `empty_named_property_map_preserves_required_interoperability_streams`; zero-length GUID streams were treated as missing by libpff; ScanPST and Outlook accept current candidates

### EMP-04
- **Status:** Empirical. Retain unchanged. Do not substitute `0xC1` or remove it without human disposition and a ScanPST-first candidate.
- **Requirement:** Fixed internal node `0xEC1` is emitted as an empty search-folder template
- **Sources:** MS-PST 2.4.1 documents `NID_SEARCH_FOLDER_TEMPLATE` as `0xC1`; no Microsoft source for `0xEC1` has been located
- **Implementation:** `NID_SEARCH_FOLDER_TEMPLATE`, `node_entries`
- **Evidence:** ScanPST repaired-r6 graph introduced/retained it; later candidates are clean

### EMP-05
- **Status:** Empirical. Retain unchanged pending comparison with Outlook-created controls and human disposition.
- **Requirement:** The store PC emits Boolean property `0x6633 = true`
- **Sources:** No Microsoft property definition was located; it is absent from the MS-PST minimum and sample store property lists
- **Implementation:** `store_properties`
- **Evidence:** Present in ScanPST-clean PSTForge candidates

### EMP-06
- **Status:** Specification conflict. Retain unchanged while Outlook-created table schemas are compared. Any change requires human disposition and ScanPST-first acceptance.
- **Requirement:** Hierarchy and contents templates encode `0x0E30` as binary; hierarchy encodes `0x3613` as Unicode
- **Sources:** PST-HIERARCHY and PST-CONTENTS say `0x0E30` is `PtypInteger32`, and PST-HIERARCHY says `0x3613` is `PtypBinary`; OXPROPS-CONTAINER independently requires `0x3613` to be `PtypString`
- **Implementation:** `hierarchy_columns`, `contents_columns`, `message_table_row`
- **Evidence:** ScanPST-clean candidates and repaired-r6-derived metadata accept the current Binary/Unicode encoding

### EMP-07
- **Status:** Microsoft-source contradiction. Retain current item-count semantics pending control comparison and human disposition.
- **Requirement:** Fixed Root and IPM subtree PCs use message counts, while MS-PST 2.7.3.4.1/.2 examples put hierarchy-row counts in `PidTagContentCount` despite defining it as item count
- **Sources:** PST-FOLDER-PC defines `0x3602` as total items; MS-PST fixed-folder examples use 3 and 1 when their contents tables have zero rows
- **Implementation:** `folder_properties`, fixed Root/IPM blocks in `create_flat_store`
- **Evidence:** Current values agree with source-folder message semantics and pass ScanPST/Outlook

### EMP-08
- **Status:** Empirical interoperability output. Retain unchanged; no schema reduction or replacement without human disposition.
- **Requirement:** Receive, outgoing, contents-index, search-index, and attachment-index tables use ScanPST-derived PST schemas and fixed NIDs
- **Sources:** MS-PST does not define these PST node schemas; MAPI-RECEIVE and MAPI-OUTGOING describe provider-facing tables with different required column sets
- **Implementation:** `receive_folder_columns`, `outgoing_queue_columns`, `*_index_columns`, fixed blocks 20-24
- **Evidence:** r6-r8 ScanPST repaired references supplied the current descriptors; later candidates are clean

### EMP-09
- **Status:** Empirical values. Retain unchanged until the structures are normatively established or the owner approves the demonstrated deterministic convention.
- **Requirement:** Contents rows emit ScanPST-derived replication instance values (`0x0E30`, `0x0E33`, `0x0E34`) and the store emits a 16-byte `0x0E34`
- **Sources:** MS-PST documents the column tags but not PSTForge's deterministic values; the MS-PST sample store has a structurally different 24-byte version-history value
- **Implementation:** `message_table_row`, `message_instance_entry_id`, `store_properties`
- **Evidence:** r6-r8 repaired-reference comparison and clean later ScanPST runs

### EMP-10
- **Status:** Empirical creation-state seed. Retain unchanged. Any normalization, randomization, or alternative initialization requires human disposition and a ScanPST-first candidate.
- **Requirement:** A newly created header starts `bidUnused` at `0x0000000100000004` and `dwUnique` at `2`
- **Sources:** MS-PST 2.2.2.6 identifies `bidUnused` as unused padding and requires `dwUnique` to increase when the header changes, but does not prescribe these initial values
- **Implementation:** `UnicodeHeader::new_store`
- **Evidence:** Outlook-created control and Microsoft sample-header comparison; earlier zero/default output failed Outlook resource handling, while the current seed passes ScanPST and Outlook

### EMP-11
- **Status:** Accepted interoperability exception. Exact-length output is the approved behavior.
- **Requirement:** PtypString values are stored as exact UTF-16LE payload bytes without a terminating NUL in PC and TC allocations
- **Sources:** OXCDATA-TYPES describes PtypString with a terminating NUL; MS-PST defines HNID placement but does not state a PST-specific terminator exception
- **Implementation:** `unicode_bytes`, `property_context`, `table_variable_bytes`
- **Focused test:** `writer::tests::property_context_heap_round_trips` asserts exact UTF-16LE allocation length and the absence of a trailing NUL
- **Evidence:** Byte comparison with Outlook-compatible controls; the earlier strict-NUL candidate displayed `_` folder and `€` subject suffixes in MailPlus, while exact-length output is clean in ScanPST, Outlook, and MailPlus. The owner accepted the proven behavior for the specification-conflict case.

### EMP-12
- **Status:** Verified. The current packed representation is the explicit PST requirement, not an empirical exception.
- **Requirement:** Fixed-width multivalue properties are stored as tightly packed elements whose count is inferred from allocation length
- **Sources:** MS-PST 2.3.3.4 and 2.3.3.4.1 explicitly define packed MV storage and require fixed-width element count to be derived by dividing the heap or node allocation length by the element width. Generic MS-OXCDATA describes the logical property type and does not override the PST physical encoding.
- **Implementation:** `PropertyValue::variable_bytes`, adapted `PropertyValue::read`/`write`
- **Focused tests:** `fixed_multivalues_use_packed_pst_storage_without_a_count`; `fixed_multivalue_rejects_partial_trailing_elements`
- **Evidence:** raw-property recovery tests and ScanPST-accepted associated/PIM candidates

### EMP-13
- **Status:** Source/specification conflict. Retain the exact source values and current gate unchanged. Do not remap, discard, synthesize `0x7FF9`, or reinterpret either property until the owner chooses a disposition after source-library and Outlook-created-control comparison.
- **Requirement:** The accepted calendar-exception source and output retain `0x7FFA/PtypInteger32` and `0x7FFF/PtypBoolean`, require `0x7FFA..=0x7FFE` as linkage, and do not require `0x7FF9/PtypTime`
- **Sources:** MS-OXOCAL defines replacement time as `0x7FF9/PtypTime`, start/end as `0x7FFB`/`0x7FFC`, flags as `0x7FFD`, and hidden as `0x7FFE`; MS-OXPROPS defines `0x7FFF` as `PidTagAttachmentContactPhoto`; no Microsoft definition for the observed `0x7FFA/PtypInteger32` was located
- **Implementation:** `calendar_exception_attachment_property`, `calendar_exception_attachment_property_type_is_valid`, `calendar_exception_attachment_has_linkage`; equivalent core translation gates
- **Evidence:** The owner-provided source fixture contains all nine retained attachment properties; exact libpff source/output fingerprints match; the unrepaired output passes ScanPST and opens in Outlook

### EMP-14
- **Status:** Pending owner decision for the fallback values; accepted recovery policy for permanent bounded provenance accounting. Retain all output values unchanged until comparative evidence supports a human decision.
- **Requirement:** Missing source folder/message metadata can generate `IPF.*` folder class, `IPM.Note` message class, `(no subject)`, `Unknown Sender`, a copied sender value for the missing half of a sender pair, zero submit/delivery times, `MSGFLAG_READ`, UTF-8 Internet codepage, received-time substitutes for missing creation/modification times, and subject fallback for an absent FAI display name
- **Sources:** MS-PST requires core Folder/Message-PC properties; MAPI-CONTAINER defines the standard folder-class values and the product spec defines the UTF-8 body fallback, but no located Microsoft source requires the user-visible subject/sender substitutions or content-derived classification of a source folder whose class is absent
- **Implementation:** `translate_message`, `contained_filetime`, `default_container_class`, folder input construction, `associated_display_name`; `message_properties`; `ReconstructionCounts`; `render_recovery_log`
- **Accounting:** `recovery.log` permanently reports grouped counts for values derived from other readable source metadata and values generated because source metadata was absent or unusable. Counts are fixed typed categories, contain no source values or item identifiers, aggregate recursively across normal, associated, and embedded messages, and survive resume through private sidecar schema 1.1.0. Reconstruction alone does not mark a candidate or part partial; `partial` remains reserved for readable source data that could not be preserved.
- **Focused tests:** `preserves_senderless_appointment_in_source_calendar`; `recovery_log_is_human_readable_bounded_and_excludes_private_paths`; `resume_rejects_schema_fourteen_without_reconstruction_accounting`
- **Evidence:** Existing fallback and malformed-property tests; completed candidates pass ScanPST/Outlook. The external GroupDocs control exercises generated metadata and requires nonempty reconstruction accounting, but complete-source fixtures do not exercise every fallback.

### EMP-15
- **Status:** Pending owner decision for the fallback values; accepted recovery policy for the same permanent bounded provenance accounting as EMP-14. Retain unchanged and do not omit usable attachment content merely because optional display metadata is absent.
- **Requirement:** Missing recipient/attachment metadata can copy display name and address from each other, generate `Recovered attachment {index}` or `Embedded message {index}.msg`, add `message/rfc822` to an embedded attachment, and default rendering position to `-1` and flags to `0`
- **Sources:** PST-RECIP-TC and PST-ATTACH-PC require the structural rows/properties; MS-OXCMSG defines `-1` as no rendering position and method 5 for embedded objects, but the generated human-visible names and MIME label are recovery policy rather than recovered source facts
- **Implementation:** recipient translation, `expected_recipient_property_value`, `translate_attachment`, `attachment_properties`; `ReconstructionCounts`; `render_recovery_log`
- **Accounting:** Derived recipient halves and generated attachment filename, MIME type, rendering position, and flags are counted without logging their values. Nested attachment/message counts merge into the owning part and do not by themselves set `partial`.
- **Evidence:** Recipient reconstruction and attachment fallback tests; exact complete-source fingerprints; Outlook/MailPlus attachment acceptance; bounded recovery-log regression

## Audit Exit Gate

Checkpoint 9 is complete only when:

1. Every `Pending` or `Partial` entry is split or resolved so each invariant has
   an exact section/property reference and an explicit result.
2. Every `Conflict` has a focused regression, clean adversarial review, full
   required local gate, and ScanPST-first human acceptance when bytes change.
3. Every `Empirical` entry has documented interoperability evidence,
   data-preservation impact, and concrete options presented to the human owner.
4. The human owner decides any removal, disabling, or narrowing of existing
   empirical output before such a code change is made.
5. Remaining 0.4.2 writer work adds or updates its entries before
   implementation.
