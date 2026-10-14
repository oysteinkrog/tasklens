# hk6.11 design: Services cells (Fable, 2026-10-14, adopted by the leader)

1. Status dot: add CellOut.dot: Option<DotStyle> (table/cells.rs:62-88), painted in the cell loop at tree_table.rs:829 with cell_colors (pages/sys_status.rs:227).
2. Selected danger colour: add CellOut.selected_text_color: Option<Hsla>, honoured in cell_text_color (cells.rs:326-336). Precedent CellSuffix { color, selected_color } (cells.rs:92-98) and tone_color (sys_status.rs:201-213).
3. Group warning icon: pass the whole NameCell to paint_group_header_cell (cells.rs:1074; tree_table.rs:755-758); draw glyphs (14 px icon, status.warning) and secondary in text_secondary after the label. No new NameCell fields.
4. Row accessible names: pub fn accessible_name(&self, ix) -> Option<String> on the delegate, like startup/delegate.rs:471 and connections/delegate.rs:491. No trait change.
5. Per-manager counts: ServicesSnapshot.manager_counts: [u32; 2] beside managers (tl-model/src/snapshot.rs:485), filled from members.len() at tl-engine/src/reducer/sys_table.rs:262 (collapsed groups drop rows at :270, so the UI cannot count).
6. Pending tag: ServicesDelegate::set_pending(UnitRef, Option<ActionKindTag>) on a FxHashMap that bumps version (CellTextCache key, tree_table.rs:113). Precedent setters services/delegate.rs:269, connections/delegate.rs:402. The Services page bead (E27-A2) calls it from PendingChanged (sys_table.rs:523).
7. UnitKind in the bead text means UnitType (tl-model/src/table.rs:186).
