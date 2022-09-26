//! remember, the d in systemd stands for demented
use std::{collections::BTreeMap, fmt::format, process::{abort, exit}};

use clap::{arg, value_parser, ArgAction, ArgGroup, Command, Parser, ValueEnum};
use console::StyledObject;
use futures::future::join_all;
use itertools::Itertools;
use regex::Regex;
use zbus::{zvariant::OwnedObjectPath, Connection, ProxyDefault};
use zbus_systemd::systemd1::{ManagerProxy, UnitProxy};

#[derive(Parser, Debug)]
#[clap(version, about, long_about = None)]
/*#[clap(group(
    ArgGroup::new("daemon-args")
        .args(&["daemon", "user_only", "system_only"]),
))]*/
struct ArgSpec {
    #[clap(long = "debug-colors")]
    debug_colors: bool,

    #[clap(long = "debug-clap", env = "DEBUG_CLAP")]
    debug_clap: bool,

    #[clap(short, long)]
    force: bool,

    #[clap(long)]
    runtime: bool,

    #[clap(long)]
    global: bool,

    #[clap(short, long)]
    multi: bool,

    #[clap(short = '1', long)]
    enable: bool,

    #[clap(short = '0', long)]
    disable: bool,

    #[clap(short = 'S', long)]
    start: bool,
    #[clap(short = 'K', long)]
    stop: bool,
    #[clap(short = 'R', long)]
    restart: bool,

    #[clap(short = 'Q', long)]
    status: bool,
    #[clap(short = 'L', long)]
    journal: bool,

    #[clap(short = 'r', action=ArgAction::Count)]
    reload_shorthand: u8,

    #[clap(long)]
    reload: bool,

    #[clap(long)]
    daemon_reload: bool,

    #[clap(action = ArgAction::Append)]
    patterns: Vec<Regex>,

    #[clap(short = 'F', action = ArgAction::Append)]
    fixed_strings: Vec<String>,

    #[clap(short = 't', long = "--type", value_enum)]
    types: Vec<TypeOpt>,

    #[clap(long = "system")]
    system_only: bool,
    #[clap(long = "user")]
    user_only: bool,

    #[clap(short = 'd', value_enum, default_value_t = DaemonOpt::Either)]
    daemon: DaemonOpt,

    #[clap(short = 'v', long = "verbose", action=ArgAction::Count)]
    verbose: u8,

    #[clap(short = 'q', long = "quiet")]
    quiet: bool,
}

#[derive(Debug, ValueEnum, Clone)]
enum DaemonOpt {
    #[clap(alias("u"))]
    User,
    #[clap(alias("s"))]
    System,
    Either,
}

#[derive(Debug, ValueEnum, Clone, strum::EnumString, strum::Display, PartialEq, Eq, PartialOrd, Ord)]
#[strum(serialize_all="lowercase")]
#[strum(ascii_case_insensitive)]
enum TypeOpt {
    #[clap(name=".")]
    _Any,

    Socket,
    #[clap(alias="s")]
    Service,
    Network,
    Device,
    Timer,
    Target,
    Slice,
    Scope,
    Mount,
    Swap,
    Path,
}

impl TypeOpt {
    fn color_str(&self, with_dot : bool) -> StyledObject<String>{
        let unit_type_str = console::style( format!("{}{}", if with_dot {"."} else {""}, self) );
        match self{
            TypeOpt::Socket => unit_type_str.yellow(),
            TypeOpt::Service => unit_type_str.white().bold(),
            TypeOpt::Network => unit_type_str.red(),
            TypeOpt::Device => unit_type_str.blue(),
            TypeOpt::Timer => unit_type_str.green(),
            TypeOpt::Target => unit_type_str.magenta().bright(),
            TypeOpt::Slice => unit_type_str.yellow().dim(),
            TypeOpt::Scope => unit_type_str.dim(),
            TypeOpt::Mount => unit_type_str.magenta().dim(),
            TypeOpt::Swap => unit_type_str.cyan().dim(),
            TypeOpt::Path => unit_type_str.blue().dim(),
            _ => panic!()
        }
    }

    fn variants() -> [Self;11]{
        [
            TypeOpt::Socket,
            TypeOpt::Service,
            TypeOpt::Network,
            TypeOpt::Device,
            TypeOpt::Timer,
            TypeOpt::Target,
            TypeOpt::Slice,
            TypeOpt::Scope,
            TypeOpt::Mount,
            TypeOpt::Swap,
            TypeOpt::Path,
        ]
    }
}

#[derive(Debug, Clone)]
struct ListUnitsItem {
    name: String,
    desc: String,
    loaded: String,
    active: String,
    plugged: String,
    other_name: String,
    path: OwnedObjectPath,
    idk: u32,
    idk2: String,
    idk3: OwnedObjectPath,
}

impl
    From<(
        String,
        String,
        String,
        String,
        String,
        String,
        OwnedObjectPath,
        u32,
        String,
        OwnedObjectPath,
    )> for ListUnitsItem
{
    fn from(
        t: (
            String,
            String,
            String,
            String,
            String,
            String,
            OwnedObjectPath,
            u32,
            String,
            OwnedObjectPath,
        ),
    ) -> Self {
        ListUnitsItem {
            name: t.0,
            desc: t.1,
            loaded: t.2,
            active: t.3,
            plugged: t.4,
            other_name: t.5,
            path: t.6,
            idk: t.7,
            idk2: t.8,
            idk3: t.9,
        }
    }
}

#[tokio::main]
async fn main() {
    let mut args = ArgSpec::parse();

    if args.types.contains(&TypeOpt::_Any){
        args.types = TypeOpt::variants().to_vec();
    }

    if args.debug_clap {
        println!("{:#?}", args);
    }
    if args.debug_colors {
        for typ in TypeOpt::variants()
        {
            println!("{}", typ.color_str(false));
        }
    }

    let filters = {
        let mut fixed_patterns = args
            .fixed_strings
            .iter()
            .map(|s| {
                let escaped = regex::escape(s);
                let pattern = if !s.contains('.') {
                    format!(r"^{}\.[^.]*$", escaped)
                } else {
                    format!(r"^{}$", escaped)
                };
                regex::Regex::new(&pattern).unwrap()
            })
            .collect();

        let mut v = args.patterns.clone();
        v.append(&mut fixed_patterns);
        v
    };
    //println!("{:?}", filters);
    #[derive(Debug, Clone, Copy, strum::Display, PartialEq, Eq, PartialOrd, Ord)]
    #[strum(serialize_all = "UPPERCASE")]
    enum DaemonType {
        User,
        System,
    }

    let conns = {
        let mut system = false;
        let mut user = false;
        if args.system_only {
            system = true;
        } else if args.user_only {
            user = true;
        } else {
            match args.daemon {
                DaemonOpt::User => user = true,
                DaemonOpt::System => system = true,
                DaemonOpt::Either => {
                    user = true;
                    system = true;
                }
            }
        }

        let mut conns: Vec<(DaemonType, Connection)> = Vec::new();
        if user {
            let conn = Connection::session()
                .await
                .expect("could not connect to dbus user session");
            conns.push((DaemonType::User, conn));
        }
        if system {
            let conn = Connection::system()
                .await
                .expect("could not connect to dbus system session");
            conns.push((DaemonType::System, conn));
        }
        conns
    };

    struct Unit<'a> {
        info: ListUnitsItem,
        daemon: DaemonType,
        //conn : &'a Connection,
        manager: ManagerProxy<'a>,
        proxy: UnitProxy<'a>,
        unit_type: TypeOpt,
        base_name: String,
    }

    let type_regex = Regex::new(r"(.*)\.([^.]*)$").unwrap();

    let mut all_units : BTreeMap<DaemonType, Vec<Unit>> = Default::default();
    all_units.insert(DaemonType::User, Vec::new());
    all_units.insert(DaemonType::System, Vec::new());

    for (daemon, conn) in conns.iter() {
        let manager = ManagerProxy::new(conn).await.unwrap();
        let units = manager.list_units().await.unwrap();

        let units = units
            .into_iter()
            .filter(|unit| match args.multi {
                true => filters.iter().any(|re| re.is_match(&unit.0)),
                false => filters.iter().all(|re| re.is_match(&unit.0)),
            }).filter_map(|unit| {
                let typ = type_regex
                    .captures(&unit.0)
                    .unwrap_or_else(|| panic!("error extracting unit type {}", unit.0))
                    .get(2)
                    .unwrap()
                    .as_str();

                let typ: TypeOpt = typ
                    .try_into()
                    .unwrap_or_else(|_| panic!("invalid unit type {}", typ));
                if args.types.contains(&typ) || args.types.is_empty(){
                    Some((unit, typ))
                }else{
                    None
                }
            })
            .map(|(unit,typ)| async {
                let info: ListUnitsItem = unit.into();
                let proxy = UnitProxy::builder(conn)
                    .path(info.path.clone())
                    .unwrap()
                    .build()
                    .await
                    .unwrap();
                let base_name = type_regex
                    .captures(&info.name).unwrap()
                    .get(1).unwrap().as_str().to_string();

                Unit {
                    info,
                    daemon: *daemon,
                    unit_type : typ,

                    manager: manager.clone(),
                    proxy,
                    base_name
                }
            });

        let mut units = join_all(units).await;
        units.sort_by_key(|v|v.info.name.clone());
        all_units.get_mut(daemon).unwrap().extend(units);
    }

    if all_units.values().all(Vec::is_empty){
        println!("Filters [{}] matched no units.", filters.iter().map(|re| format!("\'{:?}\'", re)).join( if args.multi {" || "} else {" && "}));
        exit(1)
    }

    {
        use comfy_table::{
            Table, Row, Attribute as Attr, Cell, presets, ContentArrangement, ColumnConstraint
        };
        let mut table = Table::new();
        table.load_preset(presets::NOTHING);
        table.set_content_arrangement(ContentArrangement::Dynamic);

        let mut longest = 0;
        for (daemon, units) in all_units.iter() {
            for unit in units {
                let mut row = Row::default();
                //0
                row.add_cell( 
                    Cell::new(format!("{}:", daemon))
                        .add_attribute(Attr::Dim)
                );

                //1
                #[allow(clippy::iter_nth_zero)]
                row.add_cell(
                    Cell::new(
                        format!("{}-{}-{}", 
                            &unit.info.loaded.chars().nth(0).unwrap(),
                            &unit.info.active.chars().nth(0).unwrap(),
                            &unit.info.plugged.chars().nth(0).unwrap()
                        )
                    )
                    .add_attribute(Attr::Dim)
                    .add_attribute(Attr::Bold)
                );


            
                //2
                row.add_cell(
                    Cell::new(format!("{}", unit.unit_type.color_str(false))) 
                );

                //3
                row.add_cell(
                    Cell::new(format!(
                        "{}{}",
                        unit.base_name,
                        unit.unit_type.color_str(true)
                    ))
                );

                //4
                row.add_cell(
                    Cell::new(unit.base_name.to_string())
                );

                //5
                row.add_cell(
                    Cell::new(&unit.info.loaded) 
                    .add_attribute(Attr::Dim)
                );
                //6
                row.add_cell(
                    Cell::new(&unit.info.active)
                    .add_attribute(Attr::Dim)
                );
                //7
                row.add_cell(
                    Cell::new(&unit.info.plugged)
                    .add_attribute(Attr::Dim)
                );

                //8
                row.add_cell(
                    Cell::new(&unit.info.desc)
                    .add_attribute(Attr::Italic)
                );

                let mut l = console::measure_text_width(&unit.info.name);
                if args.verbose >= 2 {
                    l += console::measure_text_width(&unit.info.desc);
                }
                longest = longest.max(l);
                table.add_row(row);
            }
        }

        let width = console::Term::stdout().size_checked().unwrap_or((0,200)).1 as usize;
        let abbreviate = true; //( longest + 40 ) > width;

        // if daemon specified remove daemon column
        if conns.len() < 2 {
            table.column_mut(0).unwrap().set_constraint(ColumnConstraint::Hidden);
        }

        if args.verbose == 0 || ! abbreviate {
            table.column_mut(1).unwrap().set_constraint(ColumnConstraint::Hidden); //abbreviated
        }

        #[allow(clippy::if_same_then_else)]
        if args.types.is_empty(){
            table.column_mut(2).unwrap().set_constraint(ColumnConstraint::Hidden); //unit type
            table.column_mut(4).unwrap().set_constraint(ColumnConstraint::Hidden); //base name
        } else if args.types.len() == 1 {
            table.column_mut(2).unwrap().set_constraint(ColumnConstraint::Hidden); //unit type
            table.column_mut(3).unwrap().set_constraint(ColumnConstraint::Hidden); //full name
        }else{
            table.column_mut(3).unwrap().set_constraint(ColumnConstraint::Hidden); //full name
        }

        if args.verbose == 0 || abbreviate {
            table.column_mut(5).unwrap().set_constraint(ColumnConstraint::Hidden); //
            table.column_mut(6).unwrap().set_constraint(ColumnConstraint::Hidden); //
            table.column_mut(7).unwrap().set_constraint(ColumnConstraint::Hidden); //
        }

        if args.verbose < 2 {
            table.column_mut(8).unwrap().set_constraint(ColumnConstraint::Hidden); //
        }


        //TODO:
        // if type specified remove type from name, 
        // if type not specified or only 1 type, remove type column
        // if status specified remove status column
        // if active specified remove active column
        // if loaded specified remove loaded column

        // if wraps, and not verbose remove desc
        // if still wraps swap with abbreviated
        println!("{}", table);
    }


    let mut actions : Vec<&'static str> = Vec::new();
    if args.start {
        actions.push("Start");
    }
    if args.stop {
        actions.push("Stop");
    }
    if args.restart {
        actions.push("Restart");
    }
    if args.enable {
        actions.push("Enable");
    }
    if args.disable {
        actions.push("Disable");
    }

    #[allow(clippy::collapsible_if)]
    if ! actions.is_empty() {
        if all_units.values().flatten().count() > 1 && ! args.force {
            let sty = console::Style::new().bold();
            let actions = actions.iter().map(|a| sty.apply_to(a).to_string()).collect_vec();

            let actions_str = [
                actions[..actions.len() - 1].join(", "),
                actions[actions.len() - 1].to_string()
            ].iter().filter(|v| ! v.is_empty()).join(" and ");

            let confirm_str = all_units.iter().filter_map(|(daemon, ls)| {
                match ls.is_empty() {
                    true => None,
                    false => Some ( 
                        format!("{} {} units", ls.len(), daemon.to_string().to_lowercase())
                    )
                }
            }).join(" and ");

            let res = dialoguer::Confirm::new()
                .with_prompt(
                    format!("{} {}?", actions_str, confirm_str)
                )
                .default(false)
                .interact()
                .expect("abort");
            /*
            let names = all_units.values().flatten()
                .map(|v| v.info.name.clone())
                .collect_vec();
            let res = dialoguer::FuzzySelect::new()
                .items(&names)
                .interact().expect("abort");
            */
            println!("{:?}", res);
        }
    }


    /*
    if args.enable && args.disable {

    } else if args.enable {

    } else if args.disable {
        unit.manager.enable_unit_files(unit.proxy, args.runtime, args.force)
    }
    */
}
