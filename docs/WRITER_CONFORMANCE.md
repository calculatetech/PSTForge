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
- `Accepted mixed conformance`: the normative portion of a grouped invariant
  is verified, while an explicitly identified empirical or conflicting portion
  uses human-approved real-world behavior.
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

### PST-MV-VARIABLE
- **Source:** MS-PST MV Properties with Variable-size Base Type
- **Section:** 2.3.3.4.2
- **Page date:** 2024-04-16 protocol revision

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

### MAPI-ATTACH-RENDER
- **Source:** [PidTagRenderingPosition](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/pidtagrenderingposition-canonical-property)
- **Section:** Remarks
- **Page date:** 2022-03-24

### MAPI-ATTACH-FLAGS
- **Source:** [PidTagAttachFlags](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/pidtagattachflags-canonical-property)
- **Section:** Remarks
- **Page date:** 2022-05-31

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
- **Section:** published revision 21.0; [personal distribution-list members 2.2.2.2.1](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocntc/9604efce-a4d9-4d6c-9541-e16dc3598dc2); [one-off members 2.2.2.2.2](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocntc/48d63d90-48e1-4edf-a84b-8ccbcd3afdde); [checksum 3.1.5.11](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocntc/51b77be4-66d3-40a7-ae95-a56982af2d68); [WrappedEntryId 2.2.2.2.4.1.1](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocntc/02656215-1cb0-4b06-a077-b07e756216be); [contacts-related folders 2.2.3](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocntc/45951a5e-82b4-4d83-83e6-24abbac67947)
- **Page date:** 2025-05-20

### OXODOC
- **Source:** [MS-OXODOC Document Object Protocol](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxodoc/103007c8-5066-4bed-84e3-4465907af098)
- **Section:** revision 13.0; 2.2, 2.2.1.1-2.2.1.34, and 2.2.2.1-2.2.2.3
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
- **Implementation:** `append_data_tree*`, `append_spooled_data_tree`, the
  0.4.5 direct-source data-tree path, `externalize_large_properties`, and
  `validate_message`
- **Evidence:** `spooled_attachment_streams_across_data_tree_groups`; 0.4.5
  direct-source chunk-boundary, declared-length, interruption, and exact-hash
  tests; recursive aggregate over-limit rejection before source open;
  direct-body and embedded direct-body tests; spooled and direct inline-empty
  binary OLE tests; empty-value, preflight, and boundary tests; `pffinfo`

### NDB-04
- **Status:** Verified: sorted local NIDs, entry widths, data/subnode BIDs, nonempty SLBLOCKs, level-0 leaves, and the single permitted level-1 SIBLOCK agree. The exact 340-by-510 capacity is checked before mutation, so 173,401 entries return a bounded error instead of emitting an invalid level-2 SIBLOCK.
- **Requirement:** Subnode B-trees preserve local node identity and embedded/attachment relationships
- **Sources:** [MS-PST 2.2.2.8.3.3.1.1 SLENTRY](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/85c4d943-0779-43c5-bd98-61dc9bb5dfd6); [2.2.2.8.3.3.1.2 SLBLOCK](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/5182eb24-4b0b-4816-aa3f-719cc6e6b018); [2.2.2.8.3.3.2.1 SIENTRY](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/9e79c673-d2f4-49fb-a00b-51b08fd2d1e4); [2.2.2.8.3.3.2.2 SIBLOCK](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/729fb9bd-060a-4bbc-9b3b-8f014b487dad)
- **Implementation:** `append_subnode_tree`, `node_entries`, `build_message_blocks`
- **Evidence:** rich-mail and embedded-message roundtrips; exact maximum-depth guard; ScanPST fidelity candidates

### NDB-05
- **Status:** Verified: Unicode leaf/intermediate entry widths, 20-entry BBT/intermediate capacity, 15-entry NBT-leaf capacity, first-key parent separators, sorted keys, page levels, page BIDs, root BREFs, reference counts, and balanced non-root occupancy match. Constructors enforce the documented maximum depth.
- **Requirement:** NBT and BBT leaf/intermediate pages are sorted, bounded, linked, and rooted correctly; every BBT `cRef` equals the BBT entry's own reference plus the actual NBT, SLBLOCK, and data-tree references to that BID
- **Sources:** MS-PST 2.2.2.7.7 and child structure pages; [MS-PST 2.2.2.7.7.3.1 Reference Counts](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/3301874b-7150-4968-9a2d-1425ca494c21), page date 2024-08-20
- **Implementation:** `write_bbt`, `write_nbt`, `plan_leaf_pages`, `initial_blocks`
- **Evidence:** `btree_leaf_planning_splits_at_ms_pst_capacity`, shared-table reference-count regressions; ScanPST 19 GB parts

### NDB-06
- **Status:** Implementation checkpoint in progress. The DList allocation mode used by supported Outlook generations is verified for first offsets, recurring intervals, reserved pages, AMap self-allocation, extent bits/free counts, page conventions/checksums, and large-file layout. Version 0.4.5 moves the adapted allocation-map construction into initial writer finalization so production publication does not reopen and rewrite the PST. Deprecated PMap/FMap/FPMap pages remain at required intervals and are not used for allocation, consistent with MS-PST product behavior.
- **Requirement:** AMap/PMap/FMap/FPMap/DList pages occur at required intervals and allocation bits cover exactly written extents
- **Sources:** [MS-PST 2.2.2.7.2 AMap](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/60466ef4-af15-49b6-8413-b3a72f0e9bdb); [2.2.2.7.3 PMap](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/e0c59db8-970a-40df-9547-c136e8858291); MS-PST 2.2.2.7.4-2.2.2.7.5; [2.2.2.7.6 FPMap](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/dd913b8e-5113-4b83-a5ea-351a08b4237b); PST-PB notes 14-17
- **Implementation:** `write_fixed_pages`, `reserved_map_page_count`, `allocate_extent`, `allocation_file_eof`, construction-time allocation-map finalization
- **Evidence:** allocation-map and FPMap-boundary tests; ScanPST 4 GB parts

### NDB-07
- **Status:** Verified. PSTForge builds a structurally complete private file by construction, syncs it once, atomically renames without replacement, syncs the held destination directory, and verifies the published device/inode. Production does not reopen, hash, extract, or independently read the completed PST. Internal structural assertions and independent readers remain mandatory in focused tests, CI gates, and release acceptance. Failure before rename leaves no public part; post-rename durability uncertainty is reported distinctly.
- **Requirement:** The writer completes every documented relationship and allocation structure before file `fsync`, atomic no-clobber rename, and directory `fsync`; production publication does not depend on rereading its output
- **Sources:** PST-INT 2.6; the verified NDB/LTP/Messaging rows above; POSIX durability is a PSTForge safety requirement
- **Implementation:** `create_flat_store`, transactional construction finalization, `publish_noclobber`, `sync_published_directory`, `verify_published_destination`; `validate_completed_store`, `validate_completed_folder_store`, and `validate_with_independent_readers` are test/acceptance utilities
- **Evidence:** construction-invariant, publication, moved-directory, no-clobber, independent-reader gate, ScanPST, and Outlook tests

### NDB-08
- **Status:** Verified; the transactional writer emits each accepted message block once before final NBT, BBT, allocation-map, and header construction. Restartable mode uses bounded private batches to amortize exact finalized-size projection while exact replay fixes every part boundary. Direct mode always performs bounded structural preflight for one top-level graph and returns its exact private allocation EOF without opening an unread payload remainder. Outside the final `part_size / 16` boundary region, that private extent is the cheap admission signal; inside the boundary region, direct mode also computes the exact finalized EOF before consuming the payload. The append verifies the private EOF token without rebuilding the complete final tables. Finalization independently reconstructs the exact finalized EOF and refuses an over-limit part before publication, including the fail-closed case where an abnormal final-index ratio exceeded the measured boundary reserve. Physical message-block order is independent of final folder-table membership and NBT order. Transactional folder node identities are fixed when their paths are first observed and do not shift when a lexicographically earlier folder is discovered later in the same one-pass construction. Once a typed location/path has retained source-folder metadata in the active layout, a later damaged candidate may add a message to that folder but cannot replace its role or class; a genuinely absent typed path is still observed before projection and append.
- **Requirement:** Transactional construction may append complete message nodes and their referenced blocks in source traversal order before finalization, provided final NBT and BBT entries remain sorted and complete, each message row is assigned to the contents or associated-contents table of its actual parent folder, and a folder node identity already referenced by a streamed message remains stable when later source folders are observed. A folder is identified by its store location and full path. Retained source-folder metadata for an existing identity remains authoritative over conflicting metadata inferred from a damaged candidate, while a missing candidate destination is added before its message is projected or appended. Allocation maps cover exactly retained extents, the header references only the final roots and high-water marks, and no pre-finalized file is published. Folder hierarchy and metadata validation is independent of any recovered message, so an unrepresentable candidate cannot suppress otherwise valid or empty source folders. A provisional batch may defer finalized-size projection; its primary bound scales from the requested part size and the bytes actually allocated in the private PST, with a message-count ceiling only to bound latency for very small items. The batch is accepted only after one exact projection. If that projection exceeds the part limit, the complete batch is rolled back to its byte-for-byte private checkpoint and replayed one message at a time with exact projection so the published part remains at the last fitting message.
- **Sources:** NDB-01 through NDB-07; MS-PST 2.2.2.1 defines the NDB as nodes and blocks addressed through the NBT and BBT; MSG-02 and MAPI-FOLDERS define the folder parentage and special-folder relationships that the retained metadata represents; PST-INT 2.6 requires the final database relationships and allocation state to be internally consistent
- **Implementation:** `validate_mail_store_layout`,
  `TransactionalMailStoreWriter::begin`, `begin_batch`,
  `append_message_deferred`, `contains_folder`, `observe_folder`, retained
  writer-input folder precedence and the direct pre-projection/rollover/final
  observation sites, the 0.4.5 exact-private and boundary-exact
  one-pass direct-source append APIs and metadata/payload protocol boundaries,
  `projected_file_eof`, `rollback_batch`, `append_message`, and `finalize`;
  `write_bbt`, `write_nbt`, `write_fixed_pages`, `write_header`, and the NDB-07
  publication path remain authoritative
- **Evidence:** focused exact-private projection rollback and append parity,
  retained-role collision and streaming-folder observation regressions, the
  50 GB Debian r12 rollover and fifth-part completion,
  exact private/final boundary append and mismatch rollback, proportional
  boundary-selection tests for 64 MiB and 4 GiB limits, late-discovered
  earlier-folder identity and contents-count regression, direct-source failure rollback/reappend, batch
  rollback, interleaved source-folder and normal/associated placement,
  exact and mismatched direct projection, one-traversal prefix/remainder
  concatenation, nested-graph metadata ordering, message-atomic direct
  completion, projection metadata immutability, recursive normal/associated
  identity rejection, exact-boundary, and byte-comparison tests; independent
  `pffinfo` and `readpst`; all five 19 GB qualification parts pass ScanPST and
  open in Outlook

## LTP Structures

### LTP-01
- **Status:** Verified: signatures, client types, root HID, page-header cadence, 2-byte map alignment, allocation/free counts, offset endpoints, 3,580-byte allocation maximum, and root/bitmap fill-level ranges agree.
- **Requirement:** Heap-on-node headers, allocation maps, page maps, fill levels, HIDs, and continuation pages are structurally valid
- **Sources:** [MS-PST 2.3.1.2 HNHDR](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/8e4ae05c-3c24-4103-b7e5-ffef6f244834), 2.3.1.3-2.3.1.4 continuation headers, [2.3.1.5 HNPAGEMAP](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/291653c0-b347-4c5b-ba41-85ad780b4ba4), and 2.3.1.6; [2.6.2.1.2 allocation](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/5b30032e-8cbc-4f03-a6bd-c21a7f1c54ea)
- **Implementation:** `heap_page`, `heap_continuation_page_allocations`, `push_heap_allocation`, `fill_heap_page`, `update_heap_fill_levels`
- **Evidence:** `property_context_heap_round_trips`, `external_table_fills_every_non_final_heap_page`; ScanPST

### LTP-02
- **Status:** Verified: `bTypeBTH`, permitted key/value widths, zero/positive roots, index-level count, sorted first-key separators, PC 2/6 records, and TC row-index 4/4 records agree.
- **Requirement:** BTH headers and records use documented key/value widths and sorted keys
- **Sources:** [MS-PST 2.3.2.1 BTHHEADER](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/5a6ab19e-1f44-4def-ad64-7bd82d94bd78), 2.3.2.2 intermediate records, and 2.3.2.3 leaf records
- **Implementation:** `property_context`, `table_context*`, LTP `tree`
- **Evidence:** property-context and rich-mail roundtrips; `pffinfo`

### LTP-03
- **Status:** Verified: property tags, <=4-byte inline values, heap HIDs, >2,048-byte PSTForge subnodes, object NID/size pairs, supported scalar byte order, and packed fixed-width multivalues agree. Compact PCs retain the single-page representation. Scalable PCs place bounded values and the 2-byte-key/6-byte-value PC BTH across HN continuation pages, update root and bitmap fill levels, and expose the ordered heap pages through the node's data tree without imposing a PSTForge property-count cap. Top-level and recursively embedded message subnode roots likewise change from one SLBLOCK to documented leaf/intermediate trees when external property values exceed one leaf's 340-entry capacity. A variable property's format representability is governed by the documented subnode/data-tree and 32-bit property-size boundaries, not the obsolete 16 KiB single-page implementation limit. The current in-memory writer API separately limits materialized raw values to the core translator's 1 MiB bound and the complete top-level item graph's message, attachment, and embedded custom-property aggregate to 128 MiB so checked rejection occurs before multi-copy serialization could violate the 2 GiB RSS gate; source properties already classified as stream-capable use `SpooledPropertySpec`. Exact-length Unicode remains the accepted EMP-11 interoperability exception.
- **Requirement:** Property contexts encode each supported property type using the documented inline, heap, or subnode representation; values larger than the HN threshold use subnodes and may span data-tree blocks up to the documented property-size boundary; when the PC exceeds one HN page, HIDs identify allocations on continuation pages and the PC BTH uses as many documented leaf and intermediate levels as its records require
- **Sources:** [MS-PST 2.3.3 PC](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/294c83c6-ff92-42f5-b6b6-876c29fa9737), [2.3.3.3 PC BTH record](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/7daab6f5-ce65-437e-80d5-1b1be4088bd3), [2.3.3.5 PtypObject](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/49457d57-820e-453d-bbc0-1d192a999814), PST-PROPS, and OXCDATA-TYPES
- **Implementation:** `property_context`, `property_context_external`, `externalize_large_properties`, `append_subnode_tree_at`, `raw_property_value`
- **Evidence:** `every_supported_raw_value_round_trips`, external-property boundary tests, scalable top-level/embedded PC tests, large named-binary tests, one-shot and transactional top-level/embedded 338/339 external-property message-subnode boundaries with rollback/reappend, and whole-item aggregate-budget traversal/arithmetic boundaries; `pffinfo` accepts the combined r5 candidate and `readpst` 0.6.76 extracts its single-leaf external PC, but that reader hard-rejects the documented level-1 PC BTH header and cannot validate the second message; human acceptance confirms clean ScanPST and successful Outlook consumption of both r5 messages and the embedded child

### LTP-04
- **Status:** Verified: RowID/RowVer offsets and bits, 4/2/1-byte regions, HNID column widths, TCINFO boundaries, sorted row BTH, tight row matrix, MSB-first CEB, zero unused bits, and HID/subnode variable values agree. RowID and RowVer are TC structural fields generated from the destination row and cannot be replaced by same-ID source properties; those properties remain eligible for the message PC. External TCs place each nonempty variable value of at most 3,580 bytes in a bounded allocation on a multi-page HN and use a subnode NID only for larger values. BTH records share the same documented continuation-page allocator. This keeps small hierarchy, contents, and recipient values out of the finite 340-by-510 SLBLOCK/SIBLOCK namespace without inventing a prohibited level-2 SIBLOCK. Hierarchy, recipient, and large contents tables are therefore bounded by documented HN page-index, HID-allocation, data-tree, and subnode limits rather than one heap page or an unnecessary subnode per small value.
- **Requirement:** Table-context column descriptors, row index, existence bitmap, row matrix, and external values agree; the writer owns RowID and RowVer and untrusted copied properties cannot overwrite them; every completed table is reread before publication to prove each matrix RowID resolves through the BTH to that same row; an HNID is a heap HID when the value fits a valid HN allocation and a subnode NID otherwise; scalable hierarchy, contents, and recipient tables preserve all representable rows and variable values without an artificial aggregate single-page or small-value subnode-count limit
- **Sources:** PST-TCINFO; [MS-PST 2.3.4.2 TCOLDESC](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/3a2f63cf-bb40-4559-910c-e55ec43d9cbb); [2.3.4.4.1 Row Data](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/c48fa6b4-bfd4-49d7-80f8-8718bc4bcddc); [2.3.4.4.2 variable data](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/a8da3d66-6051-4e30-8b8c-2b7d3c373834)
- **Implementation:** `table_context*`, `message_table_row`, `associated_message_table_row`, `validate_completed_table_indexes`, `write_external_table_value`, `push_heap_allocation`, compact-to-external hierarchy/contents/associated fallback, `schema_columns`, `write_table_value`, `mark_column`
- **Evidence:** multi-page hierarchy and contents plus singleton-external normal and associated contents, reserved-row-property and completed-index regressions, rich-mail, scalable-recipient, aggregate-recipient, bitmap fill-level, and 173,600-small-value regressions; retained-spool part 7 is accepted by `pffinfo` 20231205 and `readpst` 0.6.76 extracts all three recovered messages with zero skipped; human acceptance confirms clean ScanPST and successful Outlook consumption of all three messages; the owner reports clean ScanPST analysis of all four 0.6.0 direct 50 GB qualification parts, including the 198-folder output. Release r12 parts 1, 2, 3, and 5 scanned clean. Part 4 supplied the negative evidence: ten recovered source records carried colliding structural property IDs, ScanPST rejected the resulting RowID/BTH mismatch, and the repaired reference recovered all 12,927 top-level message nodes. The focused normal/associated RowID-collision candidate generated after the fix passed ScanPST cleanly.

## Messaging Objects And Tables

### MSG-01
- **Status:** Verified for the MS-PST minimum graph and store-PC properties; non-mandatory nodes are tracked below
- **Requirement:** Every store contains the mandatory nodes, store PC, record key, IPM subtree, wastebasket, finder, and EntryIDs
- **Sources:** PST-MANDATORY; PST-STORE; PST-EID
- **Implementation:** `node_entries`, `store_properties`, `entry_id`
- **Evidence:** `scanpst_required_metadata_is_serialized`, upstream-reader roundtrip; ScanPST r2

### MSG-02
- **Status:** Accepted mixed conformance: normative folder relationships and ordinary counts are verified; fixed-root counts use EMP-07
- **Requirement:** Folder PCs and hierarchy/contents/associated table nodes agree on parentage, counts, unread state, and child rows. Contents-table membership is derived from each message node's explicit parent folder, not from physical message serialization order.
- **Sources:** PST-FOLDER-PC; MS-PST 2.4.4; MAPI-FOLDERS; MAPI-CONTENTS
- **Implementation:** `plan_folders`, `folder_properties_with_unread`, `folder_table_row_with_unread`, `MessageStreamState` parent-tagged rows, `node_entries`
- **Evidence:** nested/root folder tests, interleaved transactional folder/placement regression; Outlook 19 GB parts

### MSG-03
- **Status:** Verified for required Message-PC fields and recipient containment; optional empty attachment-table output is retained pending procedural audit
- **Requirement:** Normal message PC, recipient table, optional attachment table, and attachment subnodes are contained under one top-level message node
- **Sources:** PST-MESSAGE; PST-MESSAGE-PC
- **Implementation:** `build_message_blocks`, `message_properties`, `recipient_table_row`, `attachment_table_row`
- **Evidence:** rich-mail and embedded roundtrips; ScanPST fidelity candidates

### MSG-04
- **Status:** Accepted mixed conformance: required columns and row/PC equality are verified; conflicting table types use EMP-06
- **Requirement:** Contents-table rows use the mandatory template columns and match message PCs
- **Sources:** PST-CONTENTS; MS-PST 2.4.4.3
- **Implementation:** `contents_columns`, `message_table_row`, `set_message_size`
- **Evidence:** multi-message/index tests; ScanPST 19 GB parts

### MSG-05
- **Status:** Verified for the required template column ID/type set and multi-page recipient tables. The corrected 0.4.4 candidate passes independent readers, ScanPST, and Outlook. Optional recipient values remain class-specific.
- **Requirement:** Recipient table is always present, has required columns, and preserves recipient type and address properties. When its row index, row matrix, or variable values do not fit one heap page, the table uses the same documented multi-page HN, BTH, data-tree row matrix, and HID-or-subnode HNID values as any other TC; one heap page is not a recipient-table limit.
- **Sources:** PST-RECIP-TC; PST-MESSAGE 2.4.5.3; LTP-01; LTP-02; LTP-04
- **Implementation:** `recipient_columns`, `recipient_table_row`, `display_recipient_properties`, `table_context_external`, `build_message_blocks`
- **Evidence:** rich-mail roundtrip; prior Outlook fidelity acceptance; 0.4.4 long-value, 448-row multi-leaf, transactional rollback/reappend, and recursive embedded exact-row regressions; completed normal/associated/embedded recipient validation; corrected r2 acceptance in `pffinfo`, `readpst`, ScanPST, and Outlook

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
- **Requirement:** Associated messages are written only to the associated contents table and carry `MSGFLAG_ASSOCIATED` in both PC and table row; normal/embedded messages do not. Every copied FAI table column, including `PidTagDisplayName`, has the same value and type in the associated message PC.
- **Sources:** MAPI-FAI; MAPI-FLAGS; MAPI-CONTENTS; PST-FAI, whose `Copied?` column is `Y` for `PidTagDisplayName`
- **Implementation:** `output_message_flags`, `associated_message_table_row`, `associated_display_name`, `message_properties`, `build_message_blocks`
- **Evidence:** `root_folders_and_associated_messages_keep_their_source_placement`, derived-display-name PC/table equality regression; ScanPST/Outlook r2

### MSG-09
- **Status:** Accepted mixed conformance: populated mappings are verified; the NAMEID property context uses the scalable HN/data-tree representation from LTP-03 when its required streams and hash buckets exceed one heap page; reserved-only/empty GUID and entry sentinels use EMP-03
- **Requirement:** Named-property streams map numeric/string names and reserved/custom GUID selectors deterministically across top-level and embedded messages without imposing an artificial single-page limit on the containing property context
- **Sources:** PST-NAMEID 2.4.7, including [entry stream](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/e17e195d-0454-4b9b-b398-c9127a26a678), [string stream](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/bbf3cbf6-74f4-48f0-899d-7d79650c021f), [GUID stream](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/0f67b30c-0891-44ef-9a80-24d43ba1b28c), and [hash buckets](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/6d390cac-0a02-4a34-9a93-e04e26f149ee)
- **Implementation:** `collect_named_identities*`, `named_property_map`, `build_named_property_context`, `externalize_large_properties`, scalable `validate_mail_store_input` preflight, `validate_named_map`
- **Evidence:** named ordering, custom GUID, empty map, embedded map, 600-identity preflight, large-string stream, and scalable NAMEID property-context tests; prior ScanPST fidelity candidates; the owner reports clean ScanPST analysis of all four 0.6.0 direct 50 GB qualification parts containing the 562-identity catalog

### MSG-10
- **Status:** Verified. The 0.4.4 explicit-empty checkpoint passes independent readers, ScanPST, and Outlook. Plain Unicode body, binary HTML, valid LZFu container with header/end marker/CRC, Outlook's terminal-NUL `RAWSIZE` convention, RTF synchronization flag, native-body enumeration, and the recovered or product-default Internet code page use the documented IDs and types. Absent body representations are not fabricated. The generated code-page fallback is EMP-14.
- **Requirement:** Message bodies preserve plain text, HTML, compressed RTF, and RTF synchronization properties without synthesizing absent bodies. A readable present `PidTagBody` with an empty `PtypString` value remains a present empty Unicode property; it is not a raw-property conflict and is not collapsed into property absence.
- **Sources:** [MS-OXCMSG 2.2.1.58.1 PidTagBody](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcmsg/f4a2d583-1033-4daf-a9eb-3e03688a194c), page date 2024-02-20, defines `PidTagBody` as `PtypString` without a non-empty restriction; [MS-OXPROPS 2.618 PidTagBody](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxprops/47171b46-6ec2-4b39-94dc-58098dc71374); [MS-OXCMSG PidTagNativeBody](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcmsg/71428f1c-a004-4c05-bc8e-6a687de06a2e); [MS-OXCMSG PidTagHtml](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcmsg/bf35ff72-9a42-428c-b376-8a8928b821dc); [MS-OXRTFCP 2.1.3.1 compression header](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxrtfcp/4c589b4d-6334-418e-93fd-1c75f820e770); [PidTagRtfInSync](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/pidtagrtfinsync-canonical-property); [PidTagInternetCodepage](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/pidtaginternetcodepage-canonical-property)
- **Implementation:** `translate_message`, `message_properties`, `validate_message`, `validate_completed_message`, `rtf_container`, `rtf_container_len`
- **Evidence:** `explicit_empty_plain_body_survives_production_translation`; empty-body creation, completed-store validation, and independent reopen in `fidelity_validation_handles_empty_raw_values_and_rejects_ambiguous_inputs`; complete core/writer suites; compressed RTF and native-body tests; exact Outlook-authored compressed-RTF roundtrip; prior Outlook/MailPlus fidelity acceptance

### MSG-11
- **Status:** Partial: the generic scalar/named/raw-property preservation path is verified, and completed class families are enumerated below. Unimplemented 0.4.2 class families remain pending their own exact protocol pass. Generated missing-source metadata is isolated in EMP-14.
- **Requirement:** Arbitrary readable message classes and raw properties retain their property type/value unless a documented generated property owns the tag
- **Sources:** PST-PROPS; OXCDATA-TYPES; class-specific MS-OX* documents
- **Implementation:** `supported_message_class`, `explicit_message_property`, `raw_property_value`
- **Evidence:** all-raw-value and class checkpoint tests; ScanPST/Outlook checkpoints

### MSG-12
- **Status:** Accepted mixed conformance: documented linkage is verified; conflicting source properties use EMP-13
- **Requirement:** Calendar exception attachment properties retain documented linkage and embedded exception content
- **Sources:** [MS-OXOCAL 2.2.10 Exceptions](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocal/ad438f25-c933-44af-afbb-bb20bc876a0b); [2.2.10.1.1 PidTagAttachmentHidden](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocal/4968bd1c-eeed-4f32-8d0f-e732cee09b5d); [2.2.10.1.6 PidTagExceptionReplaceTime](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocal/22956d67-d5cb-4db2-aa49-a6f15d24de7a); [MS-OXOCAL creation example](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxocal/7d95cc80-48b7-4ad2-93fb-767b6962ff8c); [MS-OXCICAL RECURRENCE-ID](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcical/6911f0f9-a26b-44bd-be7e-0fe38059fae0)
- **Implementation:** `calendar_exception_attachment_*`, `validate_attachment_fidelity`
- **Evidence:** `calendar_exception_attachment_properties_round_trip`; exact libpff source/output fingerprint; ScanPST/Outlook accepted checkpoint

### MSG-13
- **Status:** Accepted mixed conformance: the six required objects and four unconflicted schemas are verified; hierarchy/contents types use EMP-06 and Outlook-maintained index nodes use EMP-02
- **Requirement:** Every PST contains empty hierarchy, contents, associated-contents, search-contents, recipient, and attachment table templates at their specified NIDs; each template's schema is tracked by its section-specific entry or explicit exception
- **Sources:** MS-PST 2.7.3.3; PST-HIERARCHY; PST-CONTENTS; PST-FAI; PST-SEARCH-TC; PST-RECIP-TC; PST-ATTACH-TC
- **Implementation:** `NID_*_TABLE_TEMPLATE`, `hierarchy_columns`, `contents_columns`, `associated_columns`, `search_contents_columns`, `recipient_columns`, `attachment_columns`, fixed template blocks and `node_entries`
- **Evidence:** `scanpst_required_metadata_is_serialized`; `new_store_round_trips_through_upstream_reader`; ScanPST-clean candidates

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
- **Status:** Verified for Contact objects; Personal Distribution Lists are specified by CLS-09
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

### CLS-09
- **Status:** Verified
- **Class family:** `IPM.DistList` and descendants
- **Sources:** OXOCNTC sections 2.2.2.2.1, 2.2.2.2.2, 2.2.2.2.3, 2.2.2.2.4.1.1, 2.2.2.4.2, and 2.2.3; MS-OXPROPS 2.95, 2.96, and 2.98; PST-MV-VARIABLE; OXCDATA-TYPES
- **Required properties:** PSETID_Address `{00062004-0000-0000-C000-000000000046}` LID `0x8055` is `PtypMultipleBinary` and contains One-Off EntryIDs or WrappedEntryIds. Optional LID `0x8054` is `PtypMultipleBinary`, contains the corresponding One-Off EntryIDs, and when present has the same element count and ordering. Each property is less than 15,000 bytes. Optional LID `0x804C` is the source checksum over the member-value bytes.
- **Recovery behavior:** Preserve each structurally readable member value byte-for-byte. Do not invent members, reinterpret EntryIDs, reorder arrays, or recompute a source checksum when the member bytes are unchanged. Retain a readable source folder class; derive `IPF.Contact` only when it is absent. A missing one-off mirror is valid and remains absent. If the one-off mirror is malformed, oversized, or count-mismatched, retain the readable primary member list, omit only the mirror, and report the readable omission as partial. If the primary member list is malformed or oversized, preserve the distribution-list message metadata but omit the unusable member properties and report partial recovery.
- **Implementation:** `multiple_binary_property`, `contain_distribution_list_properties`, `validate_distribution_list_properties`, `RawPropertyValue::MultipleBinary`, `PropertyValue::variable_bytes`
- **Focused tests:** bounded offset/count parser; exact variable-MV encoding and roundtrip; missing, synchronized, mismatched, malformed, and oversized distribution-list property cases
- **Independent evidence:** Exact source/output libpff property fingerprints, `pffinfo`, and `readpst` pass; ScanPST reports clean and Outlook displays the Contacts-folder list name and both members correctly.

### CLS-10
- **Status:** Verified
- **Class family:** dotted descendants of `IPM.Document`; the undotted root is not a Document object
- **Sources:** OXODOC sections 2.2, 2.2.1.1-2.2.1.34, and 2.2.2.1-2.2.2.3; OXCMSG attachment properties; PST-MV-VARIABLE
- **Required relationships:** `PidTagMessageClass` begins `IPM.Document.` and includes a source-owned file-type suffix. A Document object has at least one attachment and should not have more than one. `PidTagDisplayName` should contain the attachment name. The 34 document-specific named properties are optional and retain their documented types; `PidNameKeywords` and `PidNameDocumentParts` are `PtypMultipleString`.
- **Recovery behavior:** Preserve every readable attachment, including additional attachments rather than enforcing the non-mandatory one-attachment recommendation. Preserve the exact source class suffix, display name, attachment names/data, and document-specific properties. Do not derive document metadata from attachment content or filename. A malformed optional property is omitted alone and reported partial. A damaged Document object with no readable attachment retains its message metadata but is reported partial because the required relationship cannot be recovered. Reference and OLE attachment methods remain separate checkpoints.
- **Implementation:** `RawPropertyValue::MultipleUnicode`, `PropertyValue::MultipleUnicode`, `multiple_unicode_bytes`, `decode_multiple_unicode`, `public_keywords_are_valid`, `document_message_class`, `ReconstructedField::DocumentAttachment`, and job schema 17
- **Focused tests:** `every_supported_raw_value_round_trips`; `multiple_unicode_offsets_and_utf16_are_bounded_and_exact`; `public_keywords_limit_counts_utf16_units`; `document_class_requires_a_dotted_suffix_and_reports_missing_attachment`; `milestone_0_4_2_document_object_roundtrip_through_libpff`
- **Independent evidence:** The bounded by-value DOCX source/output pair passes exact libpff comparison for class, folder, attachment payload/metadata, and all named-property identities, types, and payloads with zero reported omissions. The external test binds the attachment to the corrected payload SHA-256. `pffexport` extracts byte-identical DOCX payloads from source and output; `unzip -t` verifies ZIP integrity and readable content-types, relationship, and document parts; the focused fixture test asserts the officeDocument relationship type and target. `pffinfo` accepts the candidate and `readpst` completes while intentionally skipping the non-mail Document object. ScanPST reports clean, Outlook displays the Document object, and Word opens the DOCX normally.

### CLS-11
- **Status:** Verified
- **Class family:** reference attachments on any supported Message object
- **Sources:** OXCMSG sections 2.2.2.7, 2.2.2.9-2.2.2.14, and 2.2.2.26-2.2.2.28; Outlook MAPI PidTagAttachMethod canonical property
- **Required relationships:** Methods `afByReference` (`2`) and `afByReferenceOnly` (`4`) carry no by-value content and identify the source with `PidTagAttachLongPathname`. Legacy MAPI `ATTACH_BY_REF_RESOLVE` (`3`) has the same data-less path contract. `afByWebReference` (`7`) carries no content, identifies the online object with the long pathname, and uses PSETID_Attachment provider and permission metadata when present.
- **Recovery behavior:** Preserve the exact method, path/URL, filename, provider, and permission values without opening a local/UNC path, resolving the reference, making a network request, or converting it to by-value data. Omit only a reference attachment whose required method/path relationship is absent or malformed and report partial recovery. Preserve unknown provider strings as source-owned data; validate documented permission values `0..=2` without inventing defaults.
- **Implementation:** `AttachmentContent::Reference`, `AttachmentReferenceSpec`, direct FFI classification from `PidTagAttachMethod`, the durable `attachment_reference` terminal, attachment-level NAMEID mapping, completed-store relationship validation, bounded canonical translation, and job schema 18
- **Focused tests:** exact methods `2`, `3`, `4`, and `7`; data-property absence; exact path/URL and web named-property roundtrip; malformed/missing path containment; contradictory by-value property accounting; no dereference
- **Independent evidence:** The bounded `v042-reference-attachments-source` source/output pair passes exact libpff comparison for all four methods, long and optional short path values, provider and permission NAMEID identities/types/payloads, filename and folder placement, zero streamed bytes, and absence of `PidTagAttachDataBinary`. The split reports zero omissions and leaves the source identity unchanged. `pffinfo` accepts the source and `readpst` completes without attempting to retrieve any target. The owner reports that the r2 candidate passes the ScanPST-first Outlook interoperability gate.

### CLS-12
- **Status:** Verified
- **Class family:** OLE storage attachments on any supported Message object
- **Sources:** OXCMSG sections 2.2.2.8, 2.2.2.9, 2.2.2.15, and 2.2.2.17; Outlook MAPI canonical properties PidTagAttachDataObject, PidTagAttachMethod, PidTagAttachTag, PidTagAttachEncoding, and PidTagAttachRendering; [MS-PST 2.2.2.1 NID](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/18d7644e-cb33-4e11-95c0-34d8a84fbff6) and [2.4.1 Special Internal NIDs](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/0510ece4-6853-4bef-8cc8-8df3468e3ff1)
- **Required relationships:** Method `afStorage`/`ATTACH_OLE` (`6`) identifies an embedded OLE object. OLE 2 storage uses property ID `0x3701` as `PtypObject`; OLE 1 OLESTREAM data can use the same property ID as `PtypBinary`. Optional `PidTagAttachTag` (`0x370A`) identifies the source application or encoding family. Optional `PidTagAttachRendering` (`0x3709`) is a WMF static rendition; dynamic OLE objects normally carry their rendering inside the object and therefore leave this property absent or empty. Optional `PidTagAttachEncoding` (`0x3702`) is independent from the attach tag. MS-PST specifies the `PtypObject` descriptor's subnode NID and byte count but does not assign that object-data subnode a published NID type. A ScanPST-clean PST authored by classic Outlook uses reserved NID type `0x09` consistently for all five such object-data subnodes. PSTForge treats `0x09` as the Outlook OLE-object-data type under an explicit empirical interoperability exception rather than generalizing it to other nodes or properties.
- **Recovery behavior:** Preserve the exact readable method-6 payload bytes and the source `PtypObject` or `PtypBinary` relationship without instantiating, executing, repairing, converting, or dereferencing the object. Preserve a complete zero-byte `PtypBinary` value inline. Treat zero-byte `PtypObject` data as malformed because its object descriptor cannot reference a valid empty PST data block. Preserve readable attach-tag, encoding, and static-rendition bytes exactly when present, including an explicitly empty rendition; never manufacture a rendition. Preserve the same readable binary metadata on complete by-value attachments because the property contracts do not restrict it to method `6`. A missing, incomplete, oversized, or malformed required payload relationship omits only that attachment and reports partial recovery. Malformed optional metadata is omitted alone and reported partial. Preserve unknown readable tag bytes as source-owned recovery data rather than interpreting them.
- **Implementation:** `AttachmentContent::Ole`, source-typed `OleDataKind`, streamed object/binary serialization, inline empty-binary serialization, dedicated Outlook OLE-object-data NID type `0x09`, chunked completed-store validation, libpff object-subnode hashing, canonical method/type dispatch, streamed optional binary metadata, raw-metadata-aware preflight sizing, attachment-local failure containment, and job schema 19
- **Focused tests:** Exact object and binary relationships; object-data NID type `0x09`; payload, tag, encoding, and rendition hashes; complete empty-binary preservation; empty-object containment; a streamed rendition above 16 KiB; absent versus explicitly empty rendition; raw-metadata aggregate size bounds; malformed optional metadata containment; large optional metadata on reference and embedded attachments omits only that property; missing/incomplete/wrongly typed payload containment; no object execution or materialization
- **Independent evidence:** Outlook-authored source `embedded email.pst` has SHA-256 `99fc6e28ca18900f54c9411cbbcd5ef6a29fa2e6e1c5b0fd2e0b573411c15f48`, is clean in ScanPST, and contains one message with five method-6 `PtypObject` attachments. Their descriptors reference NIDs `0x8029`, `0x8049`, `0x8069`, `0x8089`, and `0x80A9`, all NID type `0x09`, with exact object sizes 19,456, 18,944, 29,184, 7,168, and 7,168 bytes. `libpff`, `pffinfo`, and PSTForge read the specimen without corruption or source mutation. The external roundtrip preserves all five payload hashes, exact `0x3701`, `0x3702`, `0x3705`, `0x3709`, and `0x370A` fingerprints, and the exact compressed RTF body with no omitted attachments. Candidate r3 has SHA-256 `0e46a4a7b0c21b91da9bfd3c5b1df0b4f01d92871e25ea87ffe5d78bd5ac8c76` and independently roundtrips a 20-KiB streamed rendition plus both synthetic OLE payloads exactly through libpff; `pffinfo`, `readpst`, `pffexport`, and ScanPST accept it, but Outlook cannot open those intentionally synthetic objects. Real-source candidate r4 passes ScanPST and preserves all five payloads, but omitted the source RTF because Outlook counted a terminal NUL in `RAWSIZE`; Outlook consequently showed an empty draft. That validator gap now has focused and external regressions. The generated fixture remains supplemental structural coverage for `PtypBinary`, optional metadata, and strict CFB payload structure, but is not an application-openability oracle. ScanPST rejected r1 NID type `31` and r2 NID type `1`; both repairs discarded the OLE object payload rather than identifying a correct replacement. Focused containment and terminal-NUL RTF tests pass with fast gate `.agent/test-results/1784474293-fast` and full gate `.agent/test-results/1784474319-full`. Real-source r5 has SHA-256 `6c56871aaf0aed122c3e43516b0afd358ac0178207bcd030a7b6001d12b3744e`; `pffinfo`, `readpst`, and `pffexport` accept it with the 56,139-byte decoded RTF and all five exact object sizes present. The owner reports ScanPST clean and confirms Outlook preserves the original rendered state exactly.

## Template And Outlook-Maintained Output

These entries are deliberately separate because reader acceptance cannot
establish their format. No entry authorizes removing the existing output.

### EMP-01
- **Status:** Accepted required interoperability output. Retain unchanged.
- **Requirement:** The hierarchy map node `0xC01` contains a fixed 124-byte HMP payload copied from deterministic ScanPST output
- **Sources:** MS-PST does not define the HMP payload; ScanPST identifies HMP as an Outlook-maintained structure
- **Implementation:** `hierarchy_map`, `NID_HIERARCHY_MAP`, `node_entries`
- **Evidence:** ScanPST accepts current candidates; historical omission caused HMP repair findings

### EMP-02
- **Status:** Accepted required Outlook-maintained interoperability output. Retain unchanged.
- **Requirement:** Contents, search, and attachment index nodes use fixed NIDs `0x6B6`, `0x6D7`, and `0x6F8`, ScanPST-derived empty schemas, and reserved NID type bits that do not advance a creation counter
- **Sources:** MS-PST mandates the six table templates in MSG-13 but does not define these three additional index nodes, their schemas, or their reserved NID type
- **Implementation:** `NID_*_INDEX_TEMPLATE`, `contents_index_columns`, `search_index_columns`, `attachment_index_columns`, `update_nid_counter`
- **Evidence:** `scanpst_required_metadata_is_serialized`; ScanPST accepts current candidates

### EMP-03
- **Status:** Accepted required interoperability output. Retain both sentinels unchanged.
- **Requirement:** An empty NAMEID map includes a fixed reserved MAPI mapping and hash bucket, and a populated map with only reserved GUID sets still includes a physical 16-byte MAPI GUID stream
- **Sources:** PST-NAMEID documents the five map properties and reserved GUID selectors, but custom GUID stream entries are indexed starting at selector 3; it does not require either sentinel
- **Implementation:** `named_property_map` empty and no-custom-GUID branches
- **Evidence:** `empty_named_property_map_preserves_required_interoperability_streams`; zero-length GUID streams were treated as missing by libpff; ScanPST and Outlook accept current candidates

### EMP-04
- **Status:** Accepted required interoperability output. Retain unchanged; do not substitute `0xC1` or remove it.
- **Requirement:** Fixed internal node `0xEC1` is emitted as an empty search-folder template
- **Sources:** MS-PST 2.4.1 documents `NID_SEARCH_FOLDER_TEMPLATE` as `0xC1`; no Microsoft source for `0xEC1` has been located
- **Implementation:** `NID_SEARCH_FOLDER_TEMPLATE`, `node_entries`
- **Evidence:** ScanPST repaired-r6 graph introduced/retained it; later candidates are clean

### EMP-05
- **Status:** Accepted required interoperability output. Retain unchanged.
- **Requirement:** The store PC emits Boolean property `0x6633 = true`
- **Sources:** No Microsoft property definition was located; it is absent from the MS-PST minimum and sample store property lists
- **Implementation:** `store_properties`
- **Evidence:** Present in ScanPST-clean PSTForge candidates

### EMP-06
- **Status:** Accepted interoperability exception. Retain the proven ScanPST- and Outlook-compatible encoding unchanged where the published requirements conflict.
- **Requirement:** Hierarchy and contents templates encode `0x0E30` as binary; hierarchy encodes `0x3613` as Unicode
- **Sources:** PST-HIERARCHY and PST-CONTENTS say `0x0E30` is `PtypInteger32`, and PST-HIERARCHY says `0x3613` is `PtypBinary`; OXPROPS-CONTAINER independently requires `0x3613` to be `PtypString`
- **Implementation:** `hierarchy_columns`, `contents_columns`, `message_table_row`
- **Evidence:** ScanPST-clean candidates and repaired-r6-derived metadata accept the current Binary/Unicode encoding

### EMP-07
- **Status:** Accepted interoperability exception. Retain the proven item-count semantics where Microsoft sources contradict each other.
- **Requirement:** Fixed Root and IPM subtree PCs use message counts, while MS-PST 2.7.3.4.1/.2 examples put hierarchy-row counts in `PidTagContentCount` despite defining it as item count
- **Sources:** PST-FOLDER-PC defines `0x3602` as total items; MS-PST fixed-folder examples use 3 and 1 when their contents tables have zero rows
- **Implementation:** `folder_properties`, fixed Root/IPM blocks in `create_flat_store`
- **Evidence:** Current values agree with source-folder message semantics and pass ScanPST/Outlook

### EMP-08
- **Status:** Accepted required interoperability output. Retain unchanged; do not reduce or replace these schemas.
- **Requirement:** Receive, outgoing, contents-index, search-index, and attachment-index tables use ScanPST-derived PST schemas and fixed NIDs
- **Sources:** MS-PST does not define these PST node schemas; MAPI-RECEIVE and MAPI-OUTGOING describe provider-facing tables with different required column sets
- **Implementation:** `receive_folder_columns`, `outgoing_queue_columns`, `*_index_columns`, fixed blocks 20-24
- **Evidence:** r6-r8 ScanPST repaired references supplied the current descriptors; later candidates are clean

### EMP-09
- **Status:** Accepted deterministic interoperability convention. Retain unchanged.
- **Requirement:** Contents rows emit ScanPST-derived replication instance values (`0x0E30`, `0x0E33`, `0x0E34`) and the store emits a 16-byte `0x0E34`
- **Sources:** MS-PST documents the column tags but not PSTForge's deterministic values; the MS-PST sample store has a structurally different 24-byte version-history value
- **Implementation:** `message_table_row`, `message_instance_entry_id`, `store_properties`
- **Evidence:** r6-r8 repaired-reference comparison and clean later ScanPST runs

### EMP-10
- **Status:** Accepted deterministic interoperability convention. Retain unchanged; do not normalize, randomize, or substitute alternative initial values.
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
- **Status:** Accepted source-preservation exception. Retain the exact source values and current gate unchanged; do not remap, discard, synthesize `0x7FF9`, or reinterpret either property.
- **Requirement:** The accepted calendar-exception source and output retain `0x7FFA/PtypInteger32` and `0x7FFF/PtypBoolean`, require `0x7FFA..=0x7FFE` as linkage, and do not require `0x7FF9/PtypTime`
- **Sources:** MS-OXOCAL defines replacement time as `0x7FF9/PtypTime`, start/end as `0x7FFB`/`0x7FFC`, flags as `0x7FFD`, and hidden as `0x7FFE`; MS-OXPROPS defines `0x7FFF` as `PidTagAttachmentContactPhoto`; no Microsoft definition for the observed `0x7FFA/PtypInteger32` was located
- **Implementation:** `calendar_exception_attachment_property`, `calendar_exception_attachment_property_type_is_valid`, `calendar_exception_attachment_has_linkage`; equivalent core translation gates
- **Evidence:** The owner-provided source fixture contains all nine retained attachment properties; exact libpff source/output fingerprints match; the unrepaired output passes ScanPST and opens in Outlook

### EMP-14
- **Status:** Accepted recovery policy for source-derived values, omission of wholly missing visible identity, and permanent bounded provenance accounting
- **Requirement:** Missing source folder/message metadata can generate `IPF.*` folder class, `IPM.Note` message class, a copied sender value for the missing half of a sender pair, zero submit/delivery times, `MSGFLAG_READ`, UTF-8 Internet codepage, received-time substitutes for missing creation/modification times, and subject fallback for an absent FAI display name; a wholly missing subject or sender identity remains absent across readable message classes, while associated messages may omit subject but retain their separate nonempty display-name rule and use `(no subject)` only for that structural display name when neither source display name nor subject exists
- **Sources:** MS-PST requires core Folder/Message-PC properties; MAPI-CONTAINER defines the standard folder-class values and the product spec defines the UTF-8 body fallback, but no located Microsoft source requires the user-visible subject/sender substitutions or content-derived classification of a source folder whose class is absent
- **Implementation:** `translate_message`, `contained_filetime`, `default_container_class`, folder input construction, `associated_display_name`; `message_properties`; `ReconstructionCounts`; `render_recovery_log`
- **Accepted derivations:** Derive a folder class from a readable message class, copy the readable half of a sender name/address pair to the missing half, substitute a valid delivery time for missing creation/modification time, derive an absent associated-message display name from its readable subject, and use code page 65001 when a nonempty complete HTML byte stream strictly validates as UTF-8. These values retain usable source facts rather than inventing unrelated metadata. Nonempty ASCII HTML is also valid UTF-8 and is byte-identical under code page 65001; an empty HTML property supplies no encoding evidence and retains the generated-default classification.
- **Accounting:** `recovery.log` permanently reports grouped counts for values derived from other readable source metadata and for absent or unusable source metadata whose destination value was defaulted or deliberately left absent. Counts are fixed typed categories, contain no source values or item identifiers, aggregate recursively across normal, associated, and embedded messages, and survive resume through private sidecar schema 1.1.0. Reconstruction or omission of nonexistent metadata alone does not mark a candidate or part partial; `partial` remains reserved for readable source data that could not be preserved.
- **Focused tests:** `preserves_senderless_appointment_in_source_calendar`; `recovery_log_is_human_readable_bounded_and_excludes_private_paths`; `resume_rejects_schema_fourteen_without_reconstruction_accounting`
- **Evidence:** Existing fallback and malformed-property tests; completed candidates pass ScanPST/Outlook. The external GroupDocs control exercises missing metadata and requires nonempty bounded accounting, but complete-source fixtures do not exercise every fallback. The two-message `qualification-v042-missing-metadata-comparison-r2` candidate preserves the former literal subject/sender values in one message and omits unknown subject/sender properties in the other; `pffinfo`, `readpst`, and ScanPST accept both. Outlook leaves omitted list fields blank and supplies `(no subject)` in the opened view. MailPlus supplies `(No subject)` and leaves the sender blank; its rendering of the fabricated sender is `<Unknown@SYNTAX_ERROR>`. The owner selected omission and client-controlled presentation.

### EMP-15
- **Status:** Accepted recovery policy for source-derived recipient values, structurally derived embedded-message MIME, bounded content-derived by-value MIME, deterministic generated attachment extensions, neutral rendering/flag defaults, and permanent bounded provenance accounting
- **Requirement:** Missing recipient/attachment metadata can copy display name and address from each other, generate `Recovered attachment {index}` or `Embedded message {index}.msg`, add `message/rfc822` to an embedded attachment, and default rendering position to `-1` and flags to `0`
- **Sources:** PST-RECIP-TC and PST-ATTACH-PC require the structural rows/properties; MAPI-ATTACH-RENDER defines `-1` as not rendered through that plain-text body-position property while RTF uses its own `\objattph` mechanism, MAPI-ATTACH-FLAGS defines zero or absent flags as processed by all applications, and MS-OXCMSG defines method 5 for embedded objects. The bounded by-value allowlist follows the file headers defined by [Adobe PDF Reference 1.5 section 3.4.1](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/pdfreference1.5_v6.pdf), [W3C PNG section 5.2](https://w3c.github.io/png/#5PNG-file-signature), [W3C JPEG JFIF](https://www.w3.org/Graphics/JPEG/jfif.pdf), [GIF89a](https://www.w3.org/Graphics/GIF/spec-gif89a.txt), and [RFC 2306 TIFF header](https://www.rfc-editor.org/rfc/rfc2306#section-3.2). Office container evidence follows [ECMA-376 Part 2 Open Packaging Conventions](https://ecma-international.org/publications-and-standards/standards/ecma-376/), [MS-DOC](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-doc/ccd7b486-7881-484c-a137-51170af7cc22), [MS-XLS section 2.1.7.20](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-xls/f682f4b0-8c6b-444e-83f8-52d156f1e8ba), and [MS-PPT](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-ppt/7bd57959-d49c-48a1-940a-bde37ec93c6f).
- **Implementation:** recipient translation, `expected_recipient_property_value`, `translate_attachment`, `attachment_mime::detect`, `BlobRange`, `attachment_properties`; `ReconstructionCounts`; `render_recovery_log`
- **Accepted derivations:** Copy a readable recipient display name or address to its missing counterpart. An attachment that structurally contains an embedded Message object derives `message/rfc822` from that method-5 relationship rather than guessing from absent payload bytes. A complete by-value attachment with no source MIME type can derive PDF, PNG, JPEG JFIF, GIF, or classic TIFF from exact leading signatures. Exact ZIP signatures derive generic `application/zip`; a bounded, namespace-aware standard-ZIP parse upgrades that label only when one unique OPC package ties a supported Office main content type to its matching required DOCX, XLSX, or PPTX part. A pre-bounded CFB parse derives legacy DOC, XLS, or PPT only from one unambiguous root main stream plus the format-specific FIB, BOF, or PowerPoint record evidence. Because filename and MIME are distinct source properties, a recognized source extension can refine generic ZIP or a matching single-family CFB when no stronger evidence conflicts; extension alone never classifies arbitrary bytes. Text heuristics, truncated flat-marker prefixes, nested CFB streams, cross-wired OOXML overrides, duplicate ZIP names, and conflicting containers do not prove a subtype. When no nonempty source filename survives, payload-proven type evidence selects the supported generated extension ahead of a conflicting but preserved source MIME value; a recognized source MIME supplies the extension when payload evidence is inconclusive. Unknown by-value payloads receive `.bin`, embedded Message objects receive `.msg`, and no payload bytes or nonempty source filenames change.
- **Accounting:** Derived recipient halves and embedded-message MIME types, plus generated attachment filenames, rendering positions, and flags, are counted without logging their values. Nested attachment/message counts merge into the owning part and do not by themselves set `partial`.
- **Evidence:** Recipient reconstruction and attachment fallback tests; exact signature allowlist, generated-extension allowlist and precedence, OOXML/CFB structure, packed-range, corrupt-container, conflicting-subtype, unknown-name, neutral-rendering, and neutral-flags regressions; exact complete-source fingerprints; Outlook/MailPlus attachment acceptance; bounded recovery-log regression. The owner reports both the focused signature candidate and common-document container candidate clean in ScanPST and Outlook.

### EMP-16
- **Status:** Accepted interoperability correction. The corrected candidate passes independent readers, ScanPST, and Outlook.
- **Requirement:** `PidTagMessageSize`, embedded-message `PtypObject.cb`, and the containing attachment/message size chain count the logical data payload owned by a dynamically built object. XBLOCK/XXBLOCK and SLBLOCK/SIBLOCK index payloads that locate those data bytes are structural indirection and are not added as user-object bytes.
- **Sources:** [MS-OXPROPS PidTagMessageSize](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxprops/bb25e964-a432-4de7-baab-5f8cd0254998) defines the bytes consumed by the Message object; [MS-PST 2.4.5 Message Objects](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/1042af37-aaa4-4edc-bffd-90a1ede24188) defines its PC, recipient/attachment objects, and subnode containment; [MS-PST 2.3.3.5 PtypObject](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/49457d57-820e-453d-bbc0-1d192a999814) defines the object NID and byte count. The located specifications do not give an exact PST formula for whether internal BTree payload bytes contribute to the server-size property.
- **Implementation:** `BlockPayload::message_size_contribution`, `append_spooled_data_tree`, `build_message_blocks`, `set_message_size`, embedded `PropertyValue::Object`, and attachment/message table rows
- **Focused tests:** multi-page recipient size regression using the repaired r1 values; transactional/embedded exact-recipient validation; in-memory versus multi-block spooled top-level/embedded size equivalence
- **Evidence:** ScanPST r1 reported embedded object computed size 69,742 versus declared 112,998, invalid attachment size, an attachment-row mismatch, and a contents-row mismatch. Its repaired reference changes the embedded and containing size fields while retaining the recipient tables. Instrumentation measured exactly 43,256 bytes of external-TC data-tree/subnode-tree index payload per 448-row table; this equals the embedded size correction, and the containing message correction includes both its own and its embedded table plus their two structural subnode wrappers. The underlying Data block payloads and all 448 outer/embedded recipient rows remain unchanged.

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

EMP-01 through EMP-10 have completed this gate. The owner accepted their
current output as required interoperability behavior after comparing clean
ScanPST/Outlook results with historical repair findings and the absence of a
demonstrably better authoritative representation. EMP-13 is likewise resolved
in favor of exact, accepted source preservation where the source and published
calendar property descriptions conflict.
