//! CSV import: reading spreadsheets, mapping their columns, the preview, the
//! folders it creates, the no-overwrite rule and the report. Each import is
//! checked by reading the workspace files on disk afterwards.

mod support;

use netterm_core::import::csv::{
    suggest_mapping, ColumnTarget, CsvError, CsvImport, CsvTable, HostField, MappingError,
    PreviewOutcome, Separator,
};
use netterm_core::import::{ImportReport, ReportEntry};
use netterm_core::workspace::{HostId, Workspace};
use support::workspaces::{files, fixture_copy, read};
use tempfile::TempDir;

/// The `edit` fixture (Access/sw1, Core/core-r1, Core/core-r2.yml, Empty) as
/// a writable copy, opened.
fn workspace() -> (TempDir, std::path::PathBuf, Workspace) {
    let (temp, root) = fixture_copy("edit");
    let workspace = Workspace::open(&root).unwrap();
    (temp, root, workspace)
}

fn table(text: &str) -> CsvTable {
    CsvTable::parse(text.as_bytes()).unwrap()
}

fn field(field: HostField) -> ColumnTarget {
    ColumnTarget::Field { field }
}

fn custom(key: &str) -> ColumnTarget {
    ColumnTarget::Custom { key: key.into() }
}

fn import(text: &str, mapping: Vec<ColumnTarget>, target: &str) -> CsvImport {
    CsvImport {
        table: table(text),
        mapping,
        target: target.into(),
    }
}

/// `(line, name, reason)` of each entry.
fn entries(entries: &[ReportEntry]) -> Vec<(Option<usize>, String, Option<String>)> {
    entries
        .iter()
        .map(|e| (e.line, e.name.clone(), e.reason.clone()))
        .collect()
}

fn created(report: &ImportReport) -> Vec<String> {
    report
        .created
        .iter()
        .map(|e| e.host.as_ref().unwrap().to_string())
        .collect()
}

// --- Reading CSV files ---------------------------------------------------

#[test]
fn a_comma_separated_file_gives_its_header_and_rows() {
    let table = table("name,address\nr1,10.0.0.1\nr2,10.0.0.2\n");
    assert_eq!(table.separator, Separator::Comma);
    assert_eq!(table.headers, ["name", "address"]);
    let rows: Vec<_> = table
        .rows
        .iter()
        .map(|r| (r.line, r.values.clone()))
        .collect();
    assert_eq!(
        rows,
        [
            (2, vec!["r1".to_string(), "10.0.0.1".into()]),
            (3, vec!["r2".to_string(), "10.0.0.2".into()]),
        ]
    );
}

#[test]
fn a_semicolon_separated_file_is_recognised() {
    // As European spreadsheets save CSV, with commas inside values.
    let table = table("name;address;description\r\nr1;10.0.0.1;core, rack 4\r\n");
    assert_eq!(table.separator, Separator::Semicolon);
    assert_eq!(table.headers, ["name", "address", "description"]);
    assert_eq!(table.rows[0].values, ["r1", "10.0.0.1", "core, rack 4"]);
}

#[test]
fn quoted_values_may_hold_separators_quotes_and_line_breaks() {
    let table = table(
        "name,description,address\n\
         r1,\"edge, \"\"primary\"\"\",10.0.0.1\n\
         r2,\"two\nlines\",10.0.0.2\n\
         r3,,10.0.0.3\n",
    );
    assert_eq!(
        table.rows[0].values,
        ["r1", "edge, \"primary\"", "10.0.0.1"]
    );
    assert_eq!(table.rows[1].values, ["r2", "two\nlines", "10.0.0.2"]);
    // A row's line is where it starts.
    assert_eq!(table.rows[1].line, 3);
    assert_eq!(table.rows[2].line, 5);
    assert_eq!(table.rows[2].values, ["r3", "", "10.0.0.3"]);
}

#[test]
fn a_byte_order_mark_is_not_part_of_the_first_header() {
    let mut bytes = b"\xEF\xBB\xBFname;address\n".to_vec();
    bytes.extend_from_slice(b"r1;10.0.0.1\n");
    let table = CsvTable::parse(&bytes).unwrap();
    assert_eq!(table.headers, ["name", "address"]);
    assert_eq!(table.separator, Separator::Semicolon);
}

#[test]
fn utf16_files_with_a_byte_order_mark_are_read() {
    let text = "name,site\nr1,Málaga\n";
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let table = CsvTable::parse(&bytes).unwrap();
    assert_eq!(table.rows[0].values, ["r1", "Málaga"]);
}

#[test]
fn broken_utf16_is_not_text() {
    // An odd number of bytes after the byte-order mark.
    let bytes = [0xFF, 0xFE, b'a', 0, b'b'];
    assert!(matches!(CsvTable::parse(&bytes), Err(CsvError::NotText)));
}

#[test]
fn a_file_that_is_not_utf8_is_read_as_windows_1252() {
    // Excel's plain "CSV (comma delimited)" on a Western Windows.
    let table = CsvTable::parse(b"name,site\nr1,M\xE1laga \x80\n").unwrap();
    assert_eq!(table.rows[0].values, ["r1", "Málaga €"]);
}

#[test]
fn blank_lines_are_not_rows() {
    let table = table("name,address\n\nr1,10.0.0.1\n\n\n");
    assert_eq!(table.rows.len(), 1);
    assert_eq!(table.rows[0].line, 3);
}

#[test]
fn a_quote_that_never_closes_is_an_error_naming_its_line() {
    let error = CsvTable::parse(b"name,address\nr1,\"10.0.0.1\nr2,10.0.0.2\n").unwrap_err();
    assert!(
        matches!(error, CsvError::UnclosedQuote { line: 2 }),
        "{error:?}"
    );
}

#[test]
fn an_empty_file_is_an_error() {
    assert!(matches!(CsvTable::parse(b""), Err(CsvError::Empty)));
    assert!(matches!(
        CsvTable::parse(b"\xEF\xBB\xBF\n\n"),
        Err(CsvError::Empty)
    ));
}

// --- Suggested mapping ---------------------------------------------------

#[test]
fn common_headers_are_mapped_to_their_fields() {
    let headers: Vec<String> = [
        "Hostname",
        "Management IP",
        "Vendor",
        "Serial Number",
        "Rack Unit",
        "Tags",
        "Folder",
        "Password",
        "Owner",
        "",
    ]
    .map(String::from)
    .into();
    assert_eq!(
        suggest_mapping(&headers),
        [
            field(HostField::Name),
            field(HostField::Address),
            field(HostField::Vendor),
            field(HostField::SerialNumber),
            field(HostField::RackUnit),
            ColumnTarget::Tags,
            ColumnTarget::FolderPath,
            ColumnTarget::Ignore,
            custom("Owner"),
            ColumnTarget::Ignore,
        ]
    );
}

#[test]
fn a_field_is_suggested_for_one_column_only() {
    let headers: Vec<String> = ["ip", "address"].map(String::from).into();
    assert_eq!(
        suggest_mapping(&headers),
        [field(HostField::Address), custom("address")]
    );
}

// --- Mapping and writing -------------------------------------------------

#[test]
fn each_kind_of_mapping_lands_in_the_host_file() {
    let (_temp, root, mut workspace) = workspace();
    let csv = "hostname,ip,vendor,rack u,owner,tags,more tags,notes\n\
               edge-1,192.0.2.1,Juniper,12,noc,\"edge, wan\",dc1,ignored text\n";
    let job = import(
        csv,
        vec![
            field(HostField::Name),
            field(HostField::Address),
            field(HostField::Vendor),
            field(HostField::RackUnit),
            custom("owner"),
            ColumnTarget::Tags,
            ColumnTarget::Tags,
            ColumnTarget::Ignore,
        ],
        "Empty",
    );
    let report = job.run(&mut workspace, &[]).unwrap();
    assert_eq!(created(&report), ["Empty/edge-1.yaml"]);
    assert_eq!(
        read(&root, "Empty/edge-1.yaml"),
        "address: 192.0.2.1\n\
         vendor: Juniper\n\
         rack_unit: 12\n\
         tags:\n- edge\n- wan\n- dc1\n\
         custom:\n  owner: noc\n"
    );
    let host = workspace.host(&HostId::new("Empty/edge-1.yaml")).unwrap();
    assert_eq!(host.name, "edge-1");
    assert_eq!(host.vendor.as_deref(), Some("Juniper"));
}

#[test]
fn without_a_name_column_the_address_names_the_host() {
    let (_temp, root, mut workspace) = workspace();
    let job = import("ip\n192.0.2.7\n", vec![field(HostField::Address)], "");
    let report = job.run(&mut workspace, &[]).unwrap();
    assert_eq!(created(&report), ["192.0.2.7.yaml"]);
    assert_eq!(read(&root, "192.0.2.7.yaml"), "address: 192.0.2.7\n");
}

#[test]
fn values_are_trimmed_and_empty_cells_set_nothing() {
    let (_temp, root, mut workspace) = workspace();
    let job = import(
        "name,address,vendor,owner\n  r9 , 10.9.9.9 ,,\n",
        vec![
            field(HostField::Name),
            field(HostField::Address),
            field(HostField::Vendor),
            custom("owner"),
        ],
        "",
    );
    job.run(&mut workspace, &[]).unwrap();
    assert_eq!(read(&root, "r9.yaml"), "address: 10.9.9.9\n");
}

#[test]
fn a_mapping_needs_an_address_column() {
    let (_temp, _root, workspace) = workspace();
    let job = import("name\nr1\n", vec![field(HostField::Name)], "");
    let error = job.preview(&workspace).unwrap_err();
    assert!(matches!(
        error,
        MappingError::NoAddress {
            first_row_is_a_device: false
        }
    ));
    // It says what to do (#116).
    assert_eq!(
        error.to_string(),
        "no column fills the management address: choose Management address for the column that holds it"
    );
}

#[test]
fn a_csv_file_that_is_not_there_says_so() {
    let temp = TempDir::new().unwrap();

    let error = CsvTable::read(temp.path().join("hosts.csv")).unwrap_err();

    assert!(matches!(error, CsvError::Io(_)));
    assert_eq!(
        error.to_string(),
        "cannot read the file: there is no such file; check the path"
    );
}

#[test]
fn a_file_without_a_row_of_column_names_is_told_to_get_one() {
    // A first export's mistake: the first device is read as the column
    // names, and its address is no column's suggestion (#116).
    let (_temp, _root, workspace) = workspace();
    let table = CsvTable::parse(b"r1,10.0.0.1,ISR4321\r\nr2,10.0.0.2,ISR4321\r\n").unwrap();
    let mapping = suggest_mapping(&table.headers);
    let job = CsvImport {
        table,
        mapping,
        target: String::new(),
    };

    let error = job.preview(&workspace).unwrap_err();

    assert_eq!(
        error.to_string(),
        "no column fills the management address, and the first row looks like a device: \
         the first row must name the columns, so add one such as name,address at the top of the file"
    );
}

#[test]
fn a_mapping_refuses_a_field_twice_and_a_missing_target() {
    let (_temp, _root, workspace) = workspace();
    let twice = import(
        "a,b\nx,y\n",
        vec![field(HostField::Address), field(HostField::Address)],
        "",
    );
    assert!(matches!(
        twice.preview(&workspace),
        Err(MappingError::FieldTwice(HostField::Address))
    ));
    let custom_twice = import(
        "a,b,c\nx,y,z\n",
        vec![field(HostField::Address), custom("owner"), custom("Owner")],
        "",
    );
    assert!(matches!(
        custom_twice.preview(&workspace),
        Err(MappingError::CustomTwice(_))
    ));
    let folders_twice = import(
        "a,b,c\nx,y,z\n",
        vec![
            field(HostField::Address),
            ColumnTarget::FolderPath,
            ColumnTarget::FolderPath,
        ],
        "",
    );
    assert!(matches!(
        folders_twice.preview(&workspace),
        Err(MappingError::FolderPathTwice)
    ));
    let short = import("a,b\nx,y\n", vec![field(HostField::Address)], "");
    assert!(matches!(
        short.preview(&workspace),
        Err(MappingError::ColumnCount { .. })
    ));
    let nowhere = import("a\nx\n", vec![field(HostField::Address)], "Nowhere");
    assert!(matches!(
        nowhere.preview(&workspace),
        Err(MappingError::NoSuchFolder(_))
    ));
}

// --- Preview ---------------------------------------------------------------

#[test]
fn the_preview_shows_what_would_happen_and_writes_nothing() {
    let (_temp, root, workspace) = workspace();
    let before = files(&root);
    let job = import(
        "name,address,folder\n\
         new-1,10.1.1.1,Europe/Madrid\n\
         core-r1,10.0.0.1,\n\
         ,,\n\
         bad:name,10.1.1.3,\n",
        vec![
            field(HostField::Name),
            field(HostField::Address),
            ColumnTarget::FolderPath,
        ],
        "Core",
    );
    let preview = job.preview(&workspace).unwrap();
    assert_eq!(files(&root), before, "the preview wrote something");

    let rows: Vec<_> = preview
        .rows
        .iter()
        .map(|row| (row.line, row.name.as_str(), &row.outcome))
        .collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows[0],
        (
            2,
            "new-1",
            &PreviewOutcome::Create {
                id: HostId::new("Core/Europe/Madrid/new-1.yaml")
            }
        )
    );
    assert_eq!(
        rows[1],
        (
            3,
            "core-r1",
            &PreviewOutcome::Exists {
                id: HostId::new("Core/core-r1.yaml")
            }
        )
    );
    assert!(
        matches!(rows[2].2, PreviewOutcome::Skip { .. }),
        "{:?}",
        rows[2]
    );
    assert!(
        matches!(rows[3].2, PreviewOutcome::Fail { .. }),
        "{:?}",
        rows[3]
    );
    assert_eq!(preview.new_folders, ["Core/Europe", "Core/Europe/Madrid"]);

    let host = preview.rows[0].host.as_ref().unwrap();
    assert_eq!(host.name, "new-1");
    assert_eq!(host.address, "10.1.1.1");
    assert_eq!(host.folder, "Core/Europe/Madrid");
}

// --- Folder paths ----------------------------------------------------------

#[test]
fn a_folder_path_column_creates_folders_under_the_target() {
    let (_temp, root, mut workspace) = workspace();
    let job = import(
        "name,address,path\n\
         mad-1,10.2.0.1,Europe/Madrid\n\
         mad-2,10.2.0.2,europe\\madrid\n\
         par-1,10.2.1.1,/Europe/Paris/\n\
         top-1,10.2.2.1,\n",
        vec![
            field(HostField::Name),
            field(HostField::Address),
            ColumnTarget::FolderPath,
        ],
        "Empty",
    );
    let report = job.run(&mut workspace, &[]).unwrap();
    assert_eq!(
        created(&report),
        [
            "Empty/Europe/Madrid/mad-1.yaml",
            "Empty/Europe/Madrid/mad-2.yaml",
            "Empty/Europe/Paris/par-1.yaml",
            "Empty/top-1.yaml",
        ]
    );
    assert_eq!(
        report.created_folders,
        ["Empty/Europe", "Empty/Europe/Madrid", "Empty/Europe/Paris"]
    );
    let all = files(&root);
    for file in [
        "Empty/Europe/",
        "Empty/Europe/Madrid/",
        "Empty/Europe/Madrid/mad-1.yaml",
        "Empty/Europe/Madrid/mad-2.yaml",
        "Empty/Europe/Paris/par-1.yaml",
        "Empty/top-1.yaml",
    ] {
        assert!(
            all.contains(&file.to_string()),
            "{file} missing from {all:?}"
        );
    }
    assert!(workspace.folder("Empty/Europe/Madrid").is_some());
}

#[test]
fn existing_folders_are_reused_ignoring_case() {
    let (_temp, root, mut workspace) = workspace();
    let job = import(
        "name,address,folder\nsw9,10.3.0.9,access\n",
        vec![
            field(HostField::Name),
            field(HostField::Address),
            ColumnTarget::FolderPath,
        ],
        "",
    );
    let report = job.run(&mut workspace, &[]).unwrap();
    assert_eq!(created(&report), ["Access/sw9.yaml"]);
    assert!(report.created_folders.is_empty());
    assert!(!files(&root).contains(&"access/".to_string()));
}

#[test]
fn a_folder_path_that_cannot_be_a_folder_fails_its_row() {
    let (_temp, root, mut workspace) = workspace();
    let before = files(&root);
    let job = import(
        "name,address,folder\nr1,10.0.0.9,Europe/_secret\nr2,10.0.0.8,a:b\n",
        vec![
            field(HostField::Name),
            field(HostField::Address),
            ColumnTarget::FolderPath,
        ],
        "",
    );
    let report = job.run(&mut workspace, &[]).unwrap();
    assert!(report.created.is_empty());
    assert_eq!(report.failed.len(), 2);
    assert!(report.created_folders.is_empty());
    assert_eq!(files(&root), before);
}

// --- The no-overwrite rule -------------------------------------------------

const CORE_R1: &str = "\
name: core-r1
address: 10.0.0.1
vendor: Arista
role: core
rack_unit: 12
tags:
- core
- dc1
custom:
  contract: ACME-1234
  owner: noc
port: 22
";

fn reimport_core_r1() -> CsvImport {
    import(
        "name,address,vendor,owner\n\
         CORE-R1,10.0.0.101,Cisco,ops\n\
         core-r2,10.0.0.102,Cisco,ops\n\
         core-r3,10.0.0.103,Cisco,ops\n",
        vec![
            field(HostField::Name),
            field(HostField::Address),
            field(HostField::Vendor),
            custom("owner"),
        ],
        "Core",
    )
}

#[test]
fn existing_hosts_are_skipped_without_confirmation() {
    let (_temp, root, mut workspace) = workspace();
    let core_r2 = read(&root, "Core/core-r2.yml");
    let report = reimport_core_r1().run(&mut workspace, &[]).unwrap();

    assert_eq!(read(&root, "Core/core-r1.yaml"), CORE_R1);
    assert_eq!(read(&root, "Core/core-r2.yml"), core_r2);
    assert_eq!(created(&report), ["Core/core-r3.yaml"]);
    assert!(report.updated.is_empty());
    let skipped: Vec<_> = report
        .skipped
        .iter()
        .map(|e| (e.line, e.host.clone().unwrap()))
        .collect();
    assert_eq!(
        skipped,
        [
            (Some(2), HostId::new("Core/core-r1.yaml")),
            (Some(3), HostId::new("Core/core-r2.yml")),
        ]
    );
    for entry in &report.skipped {
        let reason = entry.reason.as_deref().unwrap();
        assert!(reason.contains("already exists"), "{reason}");
    }
}

#[test]
fn a_confirmed_host_takes_the_imported_values_and_keeps_the_rest() {
    let (_temp, root, mut workspace) = workspace();
    let core_r2 = read(&root, "Core/core-r2.yml");
    let report = reimport_core_r1()
        .run(&mut workspace, &[HostId::new("Core/core-r1.yaml")])
        .unwrap();

    assert_eq!(
        read(&root, "Core/core-r1.yaml"),
        "name: core-r1\n\
         address: 10.0.0.101\n\
         vendor: Cisco\n\
         role: core\n\
         rack_unit: 12\n\
         tags:\n- core\n- dc1\n\
         custom:\n  contract: ACME-1234\n  owner: ops\n\
         port: 22\n"
    );
    // Not confirmed: untouched.
    assert_eq!(read(&root, "Core/core-r2.yml"), core_r2);
    let updated: Vec<_> = report.updated.iter().map(|e| e.host.clone()).collect();
    assert_eq!(updated, [Some(HostId::new("Core/core-r1.yaml"))]);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(created(&report), ["Core/core-r3.yaml"]);
}

#[test]
fn a_host_named_in_its_file_counts_as_existing() {
    let (_temp, root) = fixture_copy("edit");
    std::fs::write(
        root.join("Access/router-1.yaml"),
        "name: R1\naddress: 10.0.0.9\n",
    )
    .unwrap();
    let mut workspace = Workspace::open(&root).unwrap();
    let job = import(
        "name,address\nr1,10.0.0.99\n",
        vec![field(HostField::Name), field(HostField::Address)],
        "Access",
    );
    let report = job.run(&mut workspace, &[]).unwrap();
    assert!(report.created.is_empty(), "{report:?}");
    assert_eq!(
        report.skipped[0].host,
        Some(HostId::new("Access/router-1.yaml"))
    );
    assert!(!root.join("Access/r1.yaml").exists());
}

#[test]
fn a_host_that_appears_after_the_preview_is_still_not_overwritten() {
    let (_temp, root, mut workspace) = workspace();
    let job = import(
        "name,address\nlate,10.0.0.50\n",
        vec![field(HostField::Name), field(HostField::Address)],
        "",
    );
    let preview = job.preview(&workspace).unwrap();
    assert!(matches!(
        preview.rows[0].outcome,
        PreviewOutcome::Create { .. }
    ));
    // Someone else adds it meanwhile (the watcher has not reloaded yet).
    std::fs::write(root.join("late.yml"), "address: 192.0.2.50\n").unwrap();

    let report = job.run(&mut workspace, &[]).unwrap();
    assert!(report.created.is_empty(), "{report:?}");
    assert_eq!(read(&root, "late.yml"), "address: 192.0.2.50\n");
    assert!(!root.join("late.yaml").exists());
    assert_eq!(report.skipped.len() + report.failed.len(), 1);
}

// --- The report ------------------------------------------------------------

#[test]
fn the_report_lists_created_skipped_and_failed_rows_with_reasons() {
    let (_temp, _root, mut workspace) = workspace();
    let job = import(
        "name,address,password\n\
         ok-1,10.5.0.1,hunter2\n\
         sw1,10.5.0.2,\n\
         no-address,,\n\
         ok-1,10.5.0.4,\n\
         bad/name,10.5.0.5,\n\
         extra,10.5.0.6,,surplus\n\
         ,,\n",
        vec![
            field(HostField::Name),
            field(HostField::Address),
            custom("password"),
        ],
        "Access",
    );
    let report = job.run(&mut workspace, &[]).unwrap();

    assert_eq!(created(&report), ["Access/ok-1.yaml"]);
    let skipped = entries(&report.skipped);
    // The password column, the existing host, the empty row.
    assert_eq!(skipped.len(), 3, "{skipped:?}");
    assert_eq!(skipped[0].0, None);
    assert_eq!(skipped[0].1, "password");
    assert!(skipped[0].2.as_deref().unwrap().contains("password"));
    assert_eq!((skipped[1].0, skipped[1].1.as_str()), (Some(3), "sw1"));
    assert_eq!(skipped[2].0, Some(8));

    let failed = entries(&report.failed);
    let lines: Vec<_> = failed.iter().map(|f| f.0).collect();
    assert_eq!(lines, [Some(4), Some(5), Some(6), Some(7)]);
    let reasons: Vec<_> = failed.iter().map(|f| f.2.clone().unwrap()).collect();
    assert!(reasons[0].contains("address"), "{}", reasons[0]);
    assert!(reasons[1].contains("row 2"), "{}", reasons[1]);
    assert!(reasons[2].contains("name"), "{}", reasons[2]);
    assert!(reasons[3].contains("values"), "{}", reasons[3]);
}

#[test]
fn passwords_are_never_written_whatever_the_mapping() {
    let (_temp, root, mut workspace) = workspace();
    let job = import(
        "name,address,enable secret,notes\nr1,10.6.0.1,s3cret,hello\n",
        vec![
            field(HostField::Name),
            field(HostField::Address),
            field(HostField::Description),
            custom("notes"),
        ],
        "",
    );
    let report = job.run(&mut workspace, &[]).unwrap();
    let text = read(&root, "r1.yaml");
    assert!(!text.contains("s3cret"), "{text}");
    assert_eq!(text, "address: 10.6.0.1\ncustom:\n  notes: hello\n");
    assert_eq!(entries(&report.skipped)[0].1, "enable secret");
}

#[test]
fn an_ignored_password_column_is_still_reported_as_skipped() {
    let (_temp, _root, mut workspace) = workspace();
    let job = import(
        "name,address,password,secret\nr1,10.6.0.2,hunter2,\n",
        vec![
            field(HostField::Name),
            field(HostField::Address),
            ColumnTarget::Ignore,
            ColumnTarget::Ignore,
        ],
        "",
    );
    let preview = job.preview(&workspace).unwrap();
    // Only columns that hold a password are mentioned.
    let columns: Vec<_> = preview.skipped_columns.iter().map(|c| &c.name).collect();
    assert_eq!(columns, ["password"]);
    let report = job.run(&mut workspace, &[]).unwrap();
    assert_eq!(entries(&report.skipped)[0].1, "password");
    assert_eq!(created(&report), ["r1.yaml"]);
}

#[test]
fn an_import_reloads_the_workspace_once() {
    let (_temp, _root, mut workspace) = workspace();
    let before = workspace.revision();
    let mut csv = String::from("name,address,folder\n");
    for i in 0..200 {
        csv.push_str(&format!(
            "bulk-{i},10.7.{}.{},Bulk/{}\n",
            i / 250,
            i % 250,
            i % 4
        ));
    }
    let job = import(
        &csv,
        vec![
            field(HostField::Name),
            field(HostField::Address),
            ColumnTarget::FolderPath,
        ],
        "",
    );
    let report = job.run(&mut workspace, &[]).unwrap();
    assert_eq!(report.created.len(), 200);
    assert_eq!(workspace.revision(), before + 1);
    assert_eq!(workspace.folder("Bulk").unwrap().host_count(), 200);
}

#[test]
fn an_import_that_writes_nothing_does_not_reload() {
    let (_temp, _root, mut workspace) = workspace();
    let before = workspace.revision();
    let job = import(
        "name,address\nsw1,10.0.0.1\n",
        vec![field(HostField::Name), field(HostField::Address)],
        "Access",
    );
    let report = job.run(&mut workspace, &[]).unwrap();
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(workspace.revision(), before);
}
