//! overact-pack: convert CSV or Parquet activity/attribute tables to `.overact`.
//!
//!     overact-pack ACTIVITIES [-a ATTRIBUTES] -o OUT.overact [options]
//!
//! This is the canonical packer (`uv run overact-pack` runs it). It streams
//! the activities when rows are grouped by person, so 10M persons fit in
//! about 2 bytes per segment plus the attribute columns.

use overact_ingest::clean::FIXES;
use overact_ingest::{ingest_with_report, ByteSource, Config, FileSource, Table, TimeUnit};
use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

const USAGE: &str = "\
usage: overact-pack ACTIVITIES [-a ATTRIBUTES] -o OUT [options]

Convert activity and attribute tables (CSV, TSV or Parquet) to .overact.

  ACTIVITIES               activities table: one row per activity
  -a, --attributes FILE    attributes table: one row per person
  -o, --out FILE           output .overact file (required)
  --id-col COL             person id column (default: pid)
  --act-col COL            activity label column (default: act)
  --start-col COL          start time column (default: start)
  --end-col COL            end time column (default: end)
  --duration-col COL       duration column, instead of --end-col
  --seq-col COL            sequence column used to break start ties
  --attr-id-col COL        person id column in the attributes table
  --axis START,END         time axis in minutes (default: 0,1440)
  --time-unit UNIT         minute, second or hour (default: minute)
  --columns A,B            attribute columns to keep (default: all)
  --exclude A,B            attribute columns to drop
  --f64 A,B                float columns to keep as f64 instead of f32
  --no-ids                 do not store person ids
  --infer-rows N           rows used to infer CSV types (default: 100000)
  --fraction F             keep a random share of persons (default: 1)
  --seed N                 seed for --fraction (default: 1)
  --report FILE            write every fix to this CSV file
  -q, --quiet              no progress messages
  -h, --help               show this help";

fn describe(fix: &str) -> &'static str {
    match fix {
        "dropped_missing" => "rows with a missing id, activity, start or end were dropped",
        "dropped_empty" => "rows with end <= start were dropped",
        "dropped_outside" => "rows entirely outside the time axis were dropped",
        "clipped" => "rows partly outside the time axis were clipped",
        "overlap_trimmed" => "rows overlapping an earlier row had their start moved later",
        "overlap_dropped" => "rows fully covered by an earlier row were dropped",
        "gap_filled" => {
            "gaps (including at the start or end of the day) were filled with 'unknown'"
        }
        "person_filled" => "persons with no activities got a whole day of 'unknown'",
        _ => "",
    }
}

struct Args {
    activities: String,
    attributes: Option<String>,
    out: String,
    report: Option<String>,
    quiet: bool,
    cfg: Config,
}

fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

fn parse_args(argv: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut argv = argv.peekable();
    let mut cfg = Config {
        end_col: None,
        ..Config::default()
    };
    let (mut files, mut attributes, mut out, mut report, mut quiet) =
        (vec![], None, None, None, false);
    while let Some(a) = argv.next() {
        let mut val = || argv.next().ok_or(format!("{a} needs a value"));
        let num = |v: String| v.parse().map_err(|_| format!("{a}: not a number: {v}"));
        match a.as_str() {
            "-h" | "--help" => return Ok(None),
            "-a" | "--attributes" => attributes = Some(val()?),
            "-o" | "--out" => out = Some(val()?),
            "--id-col" => cfg.id_col = val()?,
            "--act-col" => cfg.act_col = val()?,
            "--start-col" => cfg.start_col = val()?,
            "--end-col" => cfg.end_col = Some(val()?),
            "--duration-col" => cfg.duration_col = Some(val()?),
            "--seq-col" => cfg.seq_col = Some(val()?),
            "--attr-id-col" => cfg.attr_id_col = Some(val()?),
            "--axis" => {
                let v = val()?;
                let p: Vec<&str> = v.split(',').collect();
                let n = |s: &str| s.trim().parse::<i32>().ok();
                cfg.axis = match p[..] {
                    [s, e] => n(s).zip(n(e)).ok_or(format!("--axis: bad value {v}"))?,
                    _ => return Err(format!("--axis needs START,END, got {v}")),
                };
            }
            "--time-unit" => {
                cfg.time_unit = match val()?.as_str() {
                    "minute" => TimeUnit::Minute,
                    "second" => TimeUnit::Second,
                    "hour" => TimeUnit::Hour,
                    u => return Err(format!("--time-unit: unknown unit {u}")),
                }
            }
            "--columns" => cfg.columns = Some(list(&val()?)),
            "--exclude" => cfg.exclude = list(&val()?),
            "--f64" => cfg.f64_cols = list(&val()?),
            "--no-ids" => cfg.store_ids = false,
            "--infer-rows" => cfg.infer_rows = num(val()?)?,
            "--fraction" => {
                let v = val()?;
                cfg.fraction = v.parse().map_err(|_| format!("{a}: not a number: {v}"))?
            }
            "--seed" => cfg.seed = num(val()?)? as u64,
            "--report" => report = Some(val()?),
            "-q" | "--quiet" => quiet = true,
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option {s}")),
            _ => files.push(a),
        }
    }
    let [activities] = &files[..] else {
        return Err(if files.is_empty() {
            "missing the activities table".into()
        } else {
            format!("expected one activities table, got {files:?}")
        });
    };
    if cfg.end_col.is_some() && cfg.duration_col.is_some() {
        return Err("give only one of --end-col or --duration-col".into());
    }
    if cfg.end_col.is_none() && cfg.duration_col.is_none() {
        cfg.end_col = Some("end".into());
    }
    cfg.activities_name = file_name(activities);
    cfg.attributes_name = attributes.as_deref().map(file_name);
    cfg.report = report.is_some();
    Ok(Some(Args {
        activities: activities.clone(),
        attributes,
        out: out.ok_or("missing -o OUT")?,
        report,
        quiet,
        cfg,
    }))
}

/// The last component of a path (for the dataset's `meta.source`).
fn file_name(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .map_or(p.to_string(), |f| f.to_string_lossy().into_owned())
}

fn open(path: &str) -> Result<Table, String> {
    let src: Arc<dyn ByteSource> =
        Arc::new(FileSource::open(path).map_err(|e| format!("{path}: {e}"))?);
    Table::open(src, path).map_err(|e| format!("{path}: {e}"))
}

/// `YYYY-MM-DDTHH:MM:SS+00:00` for a Unix time (days-to-civil, proleptic Gregorian).
fn iso_utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn run(args: Args) -> Result<(), String> {
    let t = Instant::now();
    let say = |msg: &str| {
        if !args.quiet {
            eprintln!("[{:6.1}s] {msg}", t.elapsed().as_secs_f64());
        }
    };
    let acts = open(&args.activities)?;
    let attrs = args.attributes.as_deref().map(open).transpose()?;
    let mut last = String::new();
    let (mut ds, fixed) =
        ingest_with_report(&acts, attrs.as_ref(), &args.cfg, &mut |phase, _, _| {
            if phase != last {
                say(phase);
                last = phase.to_string();
            }
        })
        .map_err(|e| e.to_string())?;
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let meta = &mut ds.header.meta;
    meta["packer"] = format!("overact-pack {}", env!("CARGO_PKG_VERSION")).into();
    meta["created"] = iso_utc(created).into();

    let file = std::fs::File::create(&args.out).map_err(|e| format!("{}: {e}", args.out))?;
    let mut w = std::io::BufWriter::new(file);
    ds.write(&mut w).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| format!("{}: {e}", args.out))?;
    say(&format!("wrote {}", args.out));

    let h = &ds.header;
    if let Some(path) = &args.report {
        let mut r = String::from("fix,person_id,start,end,activity\n");
        if let Some(n) = h.meta["fixes"]["dropped_missing"].as_u64() {
            r.push_str(&format!("dropped_missing,,,,{n} rows (see source)\n"));
        }
        for e in &fixed {
            r.push_str(&format!(
                "{},{},{},{},{}\n",
                e.fix,
                csv_field(&e.person_id),
                e.start,
                e.end,
                csv_field(&e.activity)
            ));
        }
        std::fs::write(path, r).map_err(|e| format!("{path}: {e}"))?;
    }
    println!(
        "{} persons, {} segments, {} activities, {} columns in {:.1}s",
        h.n_persons,
        h.n_segments,
        h.activities.len(),
        h.columns.len(),
        t.elapsed().as_secs_f64()
    );
    let fixes = &h.meta["fixes"];
    if fixes.as_object().is_some_and(|f| !f.is_empty()) {
        println!("fixes:");
        for f in FIXES {
            if let Some(n) = fixes[f.name()].as_u64() {
                println!("  {:16} {n:>10}  {}", f.name(), describe(f.name()));
            }
        }
        match &args.report {
            Some(p) => println!("every fix is listed in {p}"),
            None => println!("use --report fixes.csv to list every fix"),
        }
    }
    for w in h.meta["warnings"].as_array().into_iter().flatten() {
        println!("warning: {}", w.as_str().unwrap_or_default());
    }
    Ok(())
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Ok(None) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Some(args)) => match run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("overact-pack: error: {e}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("overact-pack: error: {e}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Result<Option<Args>, String> {
        parse_args(s.split_whitespace().map(String::from))
    }

    #[test]
    fn parses_flags() {
        let a = args("acts.csv -a dir/attrs.parquet -o out.overact --duration-col dur --axis 240,1680 --exclude x,y --f64 inc --no-ids")
            .unwrap()
            .unwrap();
        assert_eq!(a.cfg.duration_col.as_deref(), Some("dur"));
        assert_eq!(a.cfg.end_col, None);
        assert_eq!(a.cfg.axis, (240, 1680));
        assert_eq!(a.cfg.exclude, ["x", "y"]);
        assert_eq!(a.cfg.f64_cols, ["inc"]);
        assert!(!a.cfg.store_ids);
        assert_eq!(a.cfg.attributes_name.as_deref(), Some("attrs.parquet"));
        let a = args("acts.csv -o o").unwrap().unwrap();
        assert_eq!(a.cfg.end_col.as_deref(), Some("end"));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(args("acts.csv").is_err());
        assert!(args("-o o").is_err());
        assert!(args("a b -o o").is_err());
        assert!(args("a -o o --end-col e --duration-col d").is_err());
        assert!(args("a -o o --bogus").is_err());
        assert!(args("a -o o --axis 5").is_err());
        assert!(args("-h").unwrap().is_none());
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00+00:00");
        assert_eq!(iso_utc(951_782_400 + 3661), "2000-02-29T01:01:01+00:00");
    }
}
