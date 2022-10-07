//! remember, the d in systemd stands for demented
use std::{collections::BTreeMap, fmt::format, process::{abort, exit}, str::FromStr};

use clap::{arg, value_parser, ArgAction, ArgGroup, Command, Parser, ValueEnum};
use console::{StyledObject, Style};
use futures::future::join_all;
use itertools::Itertools;
use regex::Regex;
use zbus::{zvariant::OwnedObjectPath, Connection, ProxyDefault, ConnectionBuilder};
use zbus_systemd::{systemd1::{ManagerProxy, UnitProxy}, zbus::Address};

#[derive(Parser, Debug)]
#[clap(version, arg_required_else_help(true), about, long_about = None)]
/*#[clap(group(
    ArgGroup::new("daemon-args")
        .args(&["daemon", "user_only", "system_only"]),
))]*/
struct ArgSpec {
    #[clap(long = "debug-colors")]
    debug_colors: bool,

    #[clap(long = "debug-clap", env = "DEBUG_CLAP")]
    debug_clap: bool,

    #[clap(long = "no-abbr")] //TODO replace
    no_abbr : bool,

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

    #[clap(short = 'Q', long, alias="query")]
    status: bool,
    #[clap(short = 'L', long, alias="logs")]
    journal: bool,

    #[clap(short = 'r', long="daemon-reload")]
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

    #[clap(short = 's', long = "state", value_enum)]
    status_filter : Vec<StatusOpt>,

    //#[clap(short = 'a', long = "and", value_enum)]
    //status_filtera : Vec<StatusOpt>,

    #[clap(short = 'x', long = "exclude", value_enum)]
    status_filterx : Vec<StatusOpt>,
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

#[derive(Debug, ValueEnum, Clone, Copy, strum::EnumString, strum::Display, PartialEq, Eq, PartialOrd, Ord)]
#[strum(serialize_all="kebab-case")]
#[strum(ascii_case_insensitive)]
enum StatusOpt{
    Loaded,
    NotFound,
    BadSetting,

    Active,
    Inactive,

    Failed,
    Dead,
    Running,
    Plugged,
    Mounted,
    Waiting,
    Exited,
    Listening,
    StatusActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum StatusOptType{
    Loaded,
    Active,
    Status,
}

impl StatusOpt {
    fn get_type(&self) -> StatusOptType{
        match self {
            StatusOpt::Active | StatusOpt::Inactive => StatusOptType::Active,
            StatusOpt::NotFound | StatusOpt::Loaded | StatusOpt::BadSetting => StatusOptType::Loaded,
            _ => StatusOptType::Status,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct StatusFields<T> {
    pub loaded : T,
    pub active : T,
    pub status : T
}

type StatusFilter = StatusFields<Vec<StatusOpt>>;
impl StatusFilter {
    fn filter_includes(&self, unit : &ListUnitsItem) -> bool{
        //match by any
        (self.loaded.is_empty() && self.active.is_empty() && self.status.is_empty()) ||
        (
            self.loaded.contains(&unit.loaded) ||
            self.active.contains(&unit.active) ||
            self.status.contains(&unit.status)
        )
    }

    /* 
    fn filter_and(&self, unit : &ListUnitsItem) -> bool{
        //match by all
        (self.loaded.is_empty() && self.active.is_empty() && self.status.is_empty()) ||
        (
            self.loaded.contains(&unit.loaded) &&
            self.active.contains(&unit.active) &&
            self.status.contains(&unit.status) 
        )
    }
    */

    fn filter_excludes(&self, unit : &ListUnitsItem) -> bool{
        //match by none
        (self.loaded.is_empty() && self.active.is_empty() && self.status.is_empty()) ||
        (
            (! self.loaded.contains(&unit.loaded)) &&
            (! self.active.contains(&unit.active)) &&
            (! self.status.contains(&unit.status))
        )
    }
}

fn split_status_list(l : &[StatusOpt]) -> StatusFilter {
    let mut ret = StatusFields::<Vec<StatusOpt>>::default();

    for state in l {
        if *state == StatusOpt::StatusActive {
            ret.status.push(StatusOpt::Active)
        }else{
            match state.get_type() {
                StatusOptType::Active => { ret.active.push(*state) }
                StatusOptType::Loaded => { ret.loaded.push(*state) }
                StatusOptType::Status => { ret.status.push(*state) }
            }
        }
    }
    ret
}

fn colorize_status(text : &str) -> Style{
    let style = Style::new();
    match text {
        "loaded" => style.dim(),
        "not-found" => style.yellow(),
        "active" => style,
        "actives" => style,
        "inactive" => style.dim(),
        "failed" => style.red(),
        "dead" => style.yellow(),
        "running" => style.green(),
        "plugged" => style,
        "mounted" => style,
        "waiting" => style.green().dim(),
        "exited" => style.dim(),
        "listening" => style.green().dim(),
        _ => style,
    }
}

#[derive(Debug, Clone)]
#[allow(unused)]
struct ListUnitsItem {
    name: String, //0
    desc: String, //1
    loaded: StatusOpt, //2
    active: StatusOpt, //3
    status: StatusOpt, //4
    other_name: String,
    path: OwnedObjectPath,
    idk: u32,
    idk2: String,
    idk3: OwnedObjectPath,
    unit_type: TypeOpt,
    base_name: String,
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
        lazy_static::lazy_static!{
            static ref TYPE_REGEX : Regex = Regex::new(r"(.*)\.([^.]*)$").unwrap();
        }

        let m = TYPE_REGEX
            .captures(&t.0)
            .unwrap_or_else(|| panic!("error extracting unit type {}", t.0));
        
        let base_name = m.get(1).unwrap().as_str().to_string();
        let unit_type = m.get(2).unwrap().as_str();
        let unit_type: TypeOpt = unit_type
            .try_into()
            .unwrap_or_else(|_| panic!("invalid unit type {}", unit_type));

        let loaded: StatusOpt = t.2.as_str()
            .try_into().ok()
            .filter(|v : &StatusOpt| (v.get_type() == StatusOptType::Loaded) )
            .unwrap_or_else(|| panic!("invalid unit loaded state {}", t.2));

        let active: StatusOpt = t.3.as_str()
            .try_into().ok()
            .filter(|v : &StatusOpt| (v.get_type() == StatusOptType::Active || *v == StatusOpt::Failed ) )
            .unwrap_or_else(|| panic!("invalid unit active state {}", t.3));

        let status: StatusOpt = t.4.as_str()
            .try_into().ok()
            .filter(|v : &StatusOpt| (v.get_type() == StatusOptType::Status|| *v == StatusOpt::Active ) )
            .unwrap_or_else(|| panic!("invalid unit status {}", t.4));

        ListUnitsItem {
            name: t.0,
            desc: t.1,
            loaded,
            active,
            status,
            other_name: t.5,
            path: t.6,
            idk: t.7,
            idk2: t.8,
            idk3: t.9,
            unit_type,
            base_name,
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

    let status_include = split_status_list(&args.status_filter);
    let status_exclude = split_status_list(&args.status_filterx);
    //let status_and = split_status_list(&args.status_filtera);

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
        if system {
            let conn = Connection::system()
                .await
                .expect("could not connect to dbus system session");
            conns.push((DaemonType::System, conn));
        }
        if user {
            if users::get_current_uid() == 0
            {
                let uid_res = std::env::var("SUDO_UID").map(|v| {
                    let n : u32 = v.parse().expect("non numeric uid");
                    n
                });
                let uid = 
                    if let Ok(uid) = uid_res && uid != 0 {
                        uid
                    } else if system{
                        //try to figure out with logind
                        let _conn = &conns[0];
                        //zbus_systemd::login1:: TODO
                        todo!()
                    } else {
                        println!("ERROR: could not determine user uid");
                        0
                    };

                
                // honor XDG_RUNTIME_DIR and DBUS_SESSION_BUS_ADDRESS
                let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_else(|_|{
                    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok()
                        .unwrap_or_else(|| format!("/run/user/{}", uid));
                    format!("unix:path={}/bus", runtime_dir)
                });
                let address : Address = Address::from_str(&address).unwrap();
                println!("connecting to {}", address);

                users::switch::set_effective_uid(uid).expect("could not set euid");
                let conn = ConnectionBuilder::address(address)
                    .expect("blarg")
                    .build()
                    .await
                    .unwrap_or_else(|e| panic!("could not connect to dbus user session (uid:{uid})\n{e}"));
                conns.push((DaemonType::User, conn));
                users::switch::set_effective_uid(0).unwrap();
            } else {
                // NOTE: i'm not sure what happens if there *is* a user session for root, like if you logged into a graphical session as root.
                let conn = Connection::session()
                    .await
                    .expect("could not connect to dbus user session");
                conns.push((DaemonType::User, conn));
            }
        }
        conns
    };

    // used for print / prompt logic only atm
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

    /*
    if filters.is_empty(){
        use clap::CommandFactory;
        ArgSpec::command().print_help().unwrap();
        exit(1);
    }
    */

    #[allow(unused)]
    struct Unit<'a> {
        info: ListUnitsItem,
        daemon: DaemonType,
        //conn : &'a Connection,
        manager: ManagerProxy<'a>,
        proxy: UnitProxy<'a>,
    }

    let mut all_units : BTreeMap<DaemonType, Vec<Unit>> = Default::default();
    all_units.insert(DaemonType::User, Vec::new());
    all_units.insert(DaemonType::System, Vec::new());

    for (daemon, conn) in conns.iter() {
        let manager = ManagerProxy::new(conn).await.unwrap();

        if args.daemon_reload {
            //todo  zbus_systemd::systemd1::Reloading
            println!("{} {daemon} daemon", console::style("reload").bold());
            manager.reload().await
                .unwrap_or_else(|e| panic!("problem reloading {daemon} daemon : {e}"))
        }

        let units = manager.list_units().await.unwrap();

        let units = units
            .into_iter()
            .map(ListUnitsItem::from)
            .filter(|unit| match args.multi {
                true => filters.iter().any(|re| re.is_match(&unit.name)),
                false => filters.iter().all(|re| re.is_match(&unit.name)),
            }).filter(|unit| {
                status_include.filter_includes(unit) && 
                status_exclude.filter_excludes(unit) //&&
                //status_and.filter_and(unit)
            }).filter(|unit| {
                args.types.contains(&unit.unit_type) || args.types.is_empty()
            })
            .map(|unit| async {
                let proxy = UnitProxy::builder(conn)
                    .path(unit.path.clone())
                    .unwrap()
                    .build()
                    .await
                    .unwrap();

                Unit {
                    info : unit,
                    daemon: *daemon,
                    manager: manager.clone(),
                    proxy,
                }
            });

        let mut units = join_all(units).await;
        units.sort_by_key(|v|v.info.name.clone());
        all_units.get_mut(daemon).unwrap().extend(units);
    }

    /* bail conditions*/
    {
        if !actions.is_empty() && filters.is_empty(){
            println!("ERROR: must specify unit or unit pattern for {}", actions.join(", "));
            exit(1);
        }
        
        //TODO more consistant logic for when to print
        if filters.is_empty() && args.daemon_reload {
            exit(1);
        }

        if all_units.values().all(Vec::is_empty){
            println!("Filters [{}] matched no units.", filters.iter().map(|re| format!("\'{:?}\'", re)).join( if args.multi {" || "} else {" && "}));
            exit(1)
        }
    }

    /* print table */
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
                            colorize_status(&unit.info.loaded.to_string()).apply_to(&unit.info.loaded.to_string().chars().nth(0).unwrap()),
                            colorize_status(&unit.info.active.to_string()).apply_to(&unit.info.active.to_string().chars().nth(0).unwrap()),
                            colorize_status(&unit.info.status.to_string()).apply_to(&unit.info.status.to_string().chars().nth(0).unwrap())
                        )
                    )
                );


            
                //2
                row.add_cell(
                    Cell::new(format!("{}", unit.info.unit_type.color_str(false))) 
                );

                //3
                row.add_cell(
                    Cell::new(format!(
                        "{}{}",
                        unit.info.base_name,
                        unit.info.unit_type.color_str(true)
                    ))
                );

                //4
                row.add_cell(
                    Cell::new(unit.info.base_name.to_string())
                );

                //5
                row.add_cell(
                    Cell::new(
                        colorize_status(&unit.info.loaded.to_string()).apply_to(&unit.info.loaded)
                    ) 
                );
                //6
                row.add_cell(
                    Cell::new(
                        colorize_status(&unit.info.active.to_string()).apply_to(&unit.info.active)
                    )
                );
                //7
                row.add_cell(
                    Cell::new(
                        colorize_status(&unit.info.status.to_string()).apply_to(&unit.info.status)
                    )
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
        let abbreviate = ( longest + 40 ) > width || args.verbose < 3;
        let abbreviate = abbreviate && ! args.no_abbr;

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


        if ! ( args.quiet && (args.force || table.row_iter().count() < 2))
        {
            println!("{}", table);
        }
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

        exit(0);
    }

    // Execute actions
    if args.enable && args.disable {
        todo!();
    } else if args.enable {
        todo!();
    } else if args.disable {
        todo!();
    }
    
}
