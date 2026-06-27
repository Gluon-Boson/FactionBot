use std::{collections::HashSet, sync::Arc, time::{SystemTime, UNIX_EPOCH}};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use rand::Rng;
use serenity::{
    async_trait,
    model::{
        channel::{Channel, PermissionOverwrite, PermissionOverwriteType},
        gateway::Ready,
        permissions::Permissions,
        prelude::*,
    },
    prelude::*,
};
use sqlx::PgPool;
use tokio::time::{Duration, sleep};
use std::fs;
use chrono::{Local, NaiveTime, Timelike};

#[derive(Deserialize, Serialize, Clone)]
struct Commodity {
    name: String,
    price: f64,
    currency: String,
    unit: String,
}

#[derive(Deserialize, Serialize)]
struct StockMarket {
    commodities: Vec<Commodity>,
}

fn load_stocks() -> StockMarket {
    match fs::read_to_string("stock.json") {
        Ok(s) => serde_json::from_str(&s).unwrap_or(StockMarket { commodities: vec![] }),
        Err(_) => StockMarket { commodities: vec![] },
    }
}

fn save_stocks(market: &StockMarket) {
    fs::write("stock.json", serde_json::to_string_pretty(market).unwrap()).unwrap();
}

struct DbPool;
impl TypeMapKey for DbPool {
    type Value = PgPool;
}

#[derive(Deserialize, Serialize)]
struct ArsenalEntry {
    country: String,
    user_id: String,
    content: String,
    timestamp: u64,
}

fn load_arsenal() -> Vec<ArsenalEntry> {
    match fs::read_to_string("arsenal.json") {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => vec![],
    }
}

fn save_arsenal(entries: &Vec<ArsenalEntry>) {
    fs::write("arsenal.json", serde_json::to_string_pretty(entries).unwrap()).unwrap();
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn to_ids(&self) -> Vec<u64> {
        match self {
            OneOrMany::One(s) => vec![s.parse().unwrap()],
            OneOrMany::Many(v) => v.iter().map(|s| s.parse().unwrap()).collect(),
        }
    }
}

#[derive(Deserialize)]
struct Member {
    id: OneOrMany,
    #[allow(dead_code)]
    country: String,
}

type MemberConfig = HashMap<String, Member>;

fn load_members() -> MemberConfig {
    let file = std::fs::read_to_string("user_id.json").expect("Could not read user_id.json");
    serde_json::from_str(&file).expect("Invalid user_id.json format")
}

#[derive(Deserialize, Serialize, Default)]
struct AutomodConfig {
    #[serde(default)]
    global: Vec<String>,
    #[serde(default)]
    users: HashMap<String, Vec<String>>,
}

fn load_automod() -> AutomodConfig {
    match fs::read_to_string("automod.json") {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => AutomodConfig::default(),
    }
}

fn save_automod(cfg: &AutomodConfig) {
    fs::write("automod.json", serde_json::to_string_pretty(cfg).unwrap()).unwrap();
}

fn automod_triggered(cfg: &AutomodConfig, user_id: u64, text: &str) -> bool {
    let lower = text.to_lowercase();
    let no_spaces = lower.replace(" ", "");

    for word in &cfg.global {
        let word_lower = word.to_lowercase();
        if lower.contains(&word_lower) || no_spaces.contains(&word_lower) {
            return true;
        }
    }

    if let Some(wildcard) = cfg.users.get("*") {
        for word in wildcard {
            let word_lower = word.to_lowercase();
            if lower.contains(&word_lower) || no_spaces.contains(&word_lower) {
                return true;
            }
        }
    }

    let uid_str = user_id.to_string();
    if let Some(words) = cfg.users.get(&uid_str) {
        for word in words {
            let word_lower = word.to_lowercase();
            if lower.contains(&word_lower) || no_spaces.contains(&word_lower) {
                return true;
            }
        }
    }

    false
}

async fn cmd_addword(ctx: &Context, msg: &Message, args: &str) {
    if msg.author.id.get() != 1103203454123511878 {
        let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await;
        return;
    }

    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() < 2 {
        let _ = msg.channel_id.say(&ctx.http, "Usage: `!addword [global|*|<user_id>] <word>`").await;
        return;
    }

    let target = parts[0].trim();
    let word   = parts[1].trim().to_lowercase();

    let mut cfg = load_automod();

    if target == "global" {
        if !cfg.global.contains(&word) {
            cfg.global.push(word.clone());
        }
    } else {
        let entry = cfg.users.entry(target.to_string()).or_default();
        if !entry.contains(&word) {
            entry.push(word.clone());
        }
    }

    save_automod(&cfg);
    let _ = msg.channel_id.say(&ctx.http, format!("✅ Added `{}` to automod list for `{}`.", word, target)).await;
}

async fn cmd_removeword(ctx: &Context, msg: &Message, args: &str) {
    if msg.author.id.get() != 1103203454123511878 {
        let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await;
        return;
    }

    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() < 2 {
        let _ = msg.channel_id.say(&ctx.http, "Usage: `!removeword [global|*|<user_id>] <word>`").await;
        return;
    }

    let target = parts[0].trim();
    let word   = parts[1].trim().to_lowercase();

    let mut cfg = load_automod();

    if target == "global" {
        cfg.global.retain(|w| w != &word);
    } else if let Some(entry) = cfg.users.get_mut(target) {
        entry.retain(|w| w != &word);
    }

    save_automod(&cfg);
    let _ = msg.channel_id.say(&ctx.http, format!("✅ Removed `{}` from automod list for `{}`.", word, target)).await;
}

async fn init_db(pool: &PgPool) {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS countries (
            name       TEXT PRIMARY KEY,
            balance    BIGINT DEFAULT 0,
            gdp        BIGINT DEFAULT 0,
            population BIGINT DEFAULT 0,
            military   BIGINT DEFAULT 0,
            resources  BIGINT DEFAULT 0
        )"
    )
    .execute(pool)
    .await
    .expect("Failed to create countries table");
}

async fn daily_income(pool: PgPool) {
    loop {
        let now = Local::now();
        let target = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        
        let seconds_until_noon = if now.time() < target {
            (target - now.time()).num_seconds()
        } else {
            let seconds_remaining_today = 86400 - now.num_seconds_from_midnight() as i64;
            seconds_remaining_today + target.num_seconds_from_midnight() as i64
        };

        sleep(Duration::from_secs(seconds_until_noon as u64)).await;

        sqlx::query("UPDATE countries SET balance = balance + gdp / 12")
            .execute(&pool)
            .await
            .ok();
        println!("Daily income applied.");
    }
}

#[derive(Deserialize, Serialize, Default)]
struct DeleteTracker {
    #[serde(default)]
    tracked_users: HashMap<String, Option<i64>>,
}

fn load_delete_tracker() -> DeleteTracker {
    match fs::read_to_string("deletetracker.json") {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => DeleteTracker::default(),
    }
}

fn save_delete_tracker(tracker: &DeleteTracker) {
    fs::write("deletetracker.json", serde_json::to_string_pretty(tracker).unwrap()).unwrap();
}

fn is_tracked(tracker: &DeleteTracker, user_id: u64) -> bool {
    let uid_str = user_id.to_string();
    
    if let Some(expiration) = tracker.tracked_users.get(&uid_str) {
        match expiration {
            None => true,
            Some(timestamp) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                now < *timestamp
            }
        }
    } else {
        false
    }
}

fn parse_duration(input: &str) -> Option<i64> {
    let input = input.trim().to_lowercase();
    
    if input == "*" {
        return Some(-1);
    }
    
    if let Some(pos) = input.find(|c: char| c.is_alphabetic()) {
        let (num_str, suffix) = input.split_at(pos);
        if let Ok(num) = num_str.parse::<i64>() {
            let seconds = match suffix {
                "s" => num,
                "m" => num * 60,
                "h" => num * 3600,
                "d" => num * 86400,
                _ => return None,
            };
            return Some(seconds);
        }
    } else {
        if let Ok(num) = input.parse::<i64>() {
            return Some(num);
        }
    }
    
    None
}

async fn cmd_deletesentmessages(ctx: &Context, msg: &Message, args: &str) {
    if msg.author.id.get() != 1103203454123511878 {
        let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await;
        return;
    }

    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.is_empty() {
        let _ = msg.channel_id.say(&ctx.http, "Usage: `!deletesentmessages @user [time|*]`\nTime: 300, 5m, 1h, 1d, or * for infinite").await;
        return;
    }

    let target_str = parts[0].trim();
    let time_str = if parts.len() > 1 { parts[1] } else { "*" };
    
    let target_id = if let Ok(id) = target_str.parse::<u64>() {
        id
    } else {
        if let Some(mention) = msg.mentions.first() {
            mention.id.get()
        } else {
            let _ = msg.channel_id.say(&ctx.http, "Usage: `!deletesentmessages @user [time|*]`").await;
            return;
        }
    };

    let duration = match parse_duration(time_str) {
        Some(d) => d,
        None => {
            let _ = msg.channel_id.say(&ctx.http, "Invalid time format. Use: 300, 5m, 1h, 1d, or *").await;
            return;
        }
    };

    let mut tracker = load_delete_tracker();
    let target_str = target_id.to_string();

    if let Some(expiration) = tracker.tracked_users.get(&target_str) {
        match expiration {
            None => {
                let _ = msg.channel_id.say(&ctx.http, "⚠️ That user is already being tracked.").await;
                return;
            }
            Some(timestamp) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                if now < *timestamp {
                    let _ = msg.channel_id.say(&ctx.http, "⚠️ That user is already being tracked.").await;
                    return;
                }
                tracker.tracked_users.remove(&target_str);
            }
        }
    }

    let expiration = if duration == -1 {
        None
    } else {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Some(now + duration)
    };

    tracker.tracked_users.insert(target_str, expiration);
    save_delete_tracker(&tracker);
    
    let time_display = if duration == -1 {
        "forever".to_string()
    } else if duration < 60 {
        format!("{} seconds", duration)
    } else if duration < 3600 {
        format!("{} minutes", duration / 60)
    } else if duration < 86400 {
        format!("{} hours", duration / 3600)
    } else {
        format!("{} days", duration / 86400)
    };
    
    let _ = msg.channel_id.say(&ctx.http, format!("✅ Now deleting messages from <@{}> for {}.", target_id, time_display)).await;
}

async fn cmd_undeletesentmessages(ctx: &Context, msg: &Message, args: &str) {
    if msg.author.id.get() != 1103203454123511878 {
        let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await;
        return;
    }

    let target_str = args.trim();
    
    let target_id = if let Ok(id) = target_str.parse::<u64>() {
        id
    } else {
        if let Some(mention) = msg.mentions.first() {
            mention.id.get()
        } else {
            let _ = msg.channel_id.say(&ctx.http, "Usage: `!undeletesentmessages @user` or `!undeletesentmessages <user_id>`").await;
            return;
        }
    };

    let mut tracker = load_delete_tracker();
    let target_str = target_id.to_string();

    if tracker.tracked_users.remove(&target_str).is_some() {
        save_delete_tracker(&tracker);
        let _ = msg.channel_id.say(&ctx.http, format!("✅ Stopped deleting messages from <@{}>.", target_id)).await;
    } else {
        let _ = msg.channel_id.say(&ctx.http, "⚠️ That user is not being tracked.").await;
    }
}

#[derive(Deserialize, Serialize, Default)]
struct IgnoreList {
    #[serde(default)]
    ignored_users: Vec<String>,
}

fn load_ignore() -> IgnoreList {
    match fs::read_to_string("ignore.json") {
        Ok(s) => {
            if let Ok(list) = serde_json::from_str::<IgnoreList>(&s) {
                return list;
            }
            IgnoreList::default()
        },
        Err(_) => IgnoreList::default(),
    }
}

fn save_ignore(list: &IgnoreList) {
    fs::write("ignore.json", serde_json::to_string_pretty(&list).unwrap()).unwrap();
}

fn is_ignored(list: &IgnoreList, user_id: u64) -> bool {
    let uid_str = user_id.to_string();
    list.ignored_users.contains(&uid_str)
}

async fn cmd_ignore(ctx: &Context, msg: &Message, args: &str) {
    if msg.author.id.get() != 1103203454123511878 {
        let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await;
        return;
    }

    let target_str = args.trim();
    
    let target_id = if let Ok(id) = target_str.parse::<u64>() {
        id
    } else {
        if let Some(mention) = msg.mentions.first() {
            mention.id.get()
        } else {
            let _ = msg.channel_id.say(&ctx.http, "Usage: `!ignore @user` or `!ignore <user_id>`").await;
            return;
        }
    };

    let mut list = load_ignore();
    let target_str = target_id.to_string();

    if list.ignored_users.contains(&target_str) {
        let _ = msg.channel_id.say(&ctx.http, "⚠️ That user is already ignored.").await;
        return;
    }

    list.ignored_users.push(target_str);
    save_ignore(&list);
    let _ = msg.channel_id.say(&ctx.http, format!("✅ Bot will now ignore <@{}>.", target_id)).await;
}

async fn cmd_unignore(ctx: &Context, msg: &Message, args: &str) {
    if msg.author.id.get() != 1103203454123511878 {
        let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await;
        return;
    }

    let target_str = args.trim();
    
    let target_id = if let Ok(id) = target_str.parse::<u64>() {
        id
    } else {
        if let Some(mention) = msg.mentions.first() {
            mention.id.get()
        } else {
            let _ = msg.channel_id.say(&ctx.http, "Usage: `!unignore @user` or `!unignore <user_id>`").await;
            return;
        }
    };

    let mut list = load_ignore();
    let target_str = target_id.to_string();

    list.ignored_users.retain(|id| id != &target_str);
    save_ignore(&list);
    let _ = msg.channel_id.say(&ctx.http, format!("✅ Bot will no longer ignore <@{}>.", target_id)).await;
}

async fn cmd_addgdp(ctx: &Context, msg: &Message, args: &str) {
    if msg.author.id.get() != 1103203454123511878 {
        let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await;
        return;
    }

    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() < 2 { return; }

    let amount: i64 = match parts[0].parse() {
        Ok(n) => n,
        Err(_) => { let _ = msg.channel_id.say(&ctx.http, "Invalid amount.").await; return; }
    };
    let target = parts[1].trim().to_string();

    let pool = {
        let data = ctx.data.read().await;
        data.get::<DbPool>().unwrap().clone()
    };

    sqlx::query("UPDATE countries SET gdp = gdp + $1 WHERE name = $2")
        .bind(amount)
        .bind(&target)
        .execute(&pool)
        .await
        .unwrap();

    let _ = msg.channel_id.say(&ctx.http, format!("✅ Added {} GDP to {}.", amount, target)).await;
}

async fn daily_stocks(http: Arc<serenity::http::Http>) {
    let channel_id = ChannelId::new(1506541126298107925);
    
    loop {
        let now = Local::now();
        let target = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        
        let seconds_until_noon = if now.time() < target {
            (target - now.time()).num_seconds()
        } else {
            let seconds_remaining_today = 86400 - now.num_seconds_from_midnight() as i64;
            seconds_remaining_today + target.num_seconds_from_midnight() as i64
        };

        sleep(Duration::from_secs(seconds_until_noon as u64)).await;

        let mut market = load_stocks();
        let mut msg = String::from("📈 **Daily Market Update:**\n");

        for commodity in &mut market.commodities {
            let change_pct = rand::thread_rng().gen_range(-5.0..=5.0_f64);
            let old_price = commodity.price;
            commodity.price = (old_price * (1.0 + change_pct / 100.0) * 100.0).round() / 100.0;
            let arrow = if commodity.price >= old_price { "🟢" } else { "🔴" };
            msg.push_str(&format!(
                "{} **{}**: {:.2} {} per {} ({:+.2}%)\n",
                arrow, commodity.name, commodity.price, commodity.currency, commodity.unit, change_pct
            ));
        }

        save_stocks(&market);
        let _ = channel_id.say(&http, msg).await;
    }
}

async fn cmd_stocks(ctx: &Context, msg: &Message) {
    let market = load_stocks();
    let mut out = String::from("📊 **Current Market Prices:**\n");
    for c in &market.commodities {
        out.push_str(&format!(
            "**{}**: {:.2} {} per {}\n",
            c.name, c.price, c.currency, c.unit
        ));
    }
    let _ = msg.channel_id.say(&ctx.http, out).await;
}

async fn cmd_savearsenal(ctx: &Context, msg: &Message) {
    let members = load_members();
    let country = members.values()
        .find(|m| m.id.to_ids().contains(&msg.author.id.get()))
        .map(|m| m.country.clone());

    let country = match country {
        Some(c) => c,
        None => { let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await; return; }
    };

    let txt_filename = msg.attachments.iter()
        .find(|a| a.filename.ends_with(".txt"))
        .map(|a| a.filename.clone())
        .unwrap_or_else(|| "entry".to_string());

    let txt_url = msg.attachments.iter()
        .find(|a| a.filename.ends_with(".txt"))
        .map(|a| a.url.clone());

    let content = if let Some(url) = txt_url {
        match reqwest::get(&url).await {
            Ok(resp) => match resp.text().await {
                Ok(t) => t,
                Err(_) => { let _ = msg.channel_id.say(&ctx.http, "Failed to read file.").await; return; }
            },
            Err(_) => { let _ = msg.channel_id.say(&ctx.http, "Failed to download file.").await; return; }
        }
    } else if !msg.content.trim_start_matches("!savearsenal").trim().is_empty() {
        msg.content.trim_start_matches("!savearsenal").trim().to_string()
    } else {
        let _ = msg.channel_id.say(&ctx.http, "Attach a .txt file or include text after the command.").await;
        return;
    };

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    let mut entries = load_arsenal();
    entries.push(ArsenalEntry {
        country: country.clone(),
        user_id: msg.author.id.get().to_string(),
        content: content.clone(),
        timestamp: now,
    });
    save_arsenal(&entries);

    let save_dir = std::env::var("SAVE_DIR").unwrap_or_else(|_| "/app/saves".to_string());
    fs::create_dir_all(&save_dir).unwrap();
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let filename = format!("{}/{}_{}_{}", save_dir, date, country, txt_filename);
    fs::write(&filename, &content).unwrap();

    let _ = msg.channel_id.say(&ctx.http, "✅ Arsenal entry saved.").await;
}

async fn cmd_checkbalance(ctx: &Context, msg: &Message) {
    let members = load_members();
    let country = members.values()
        .find(|m| m.id.to_ids().contains(&msg.author.id.get()))
        .map(|m| m.country.clone());

    let country = match country {
        Some(c) => c,
        None => { let _ = msg.channel_id.say(&ctx.http, "You don't have a country.").await; return; }
    };

    let pool = {
        let data = ctx.data.read().await;
        data.get::<DbPool>().unwrap().clone()
    };

    let balance: i64 = sqlx::query_scalar("SELECT balance FROM countries WHERE name = $1")
        .bind(&country)
        .fetch_optional(&pool)
        .await
        .unwrap()
        .unwrap_or(0);

    let _ = msg.channel_id.say(&ctx.http, format!("💰 {}: ${}", country, balance)).await;
}

async fn cmd_give(ctx: &Context, msg: &Message, args: &str) {
    if msg.author.id.get() != 1103203454123511878 {
        let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await;
        return;
    }

    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() < 2 { return; }

    let amount: i64 = match parts[0].parse() {
        Ok(n) => n,
        Err(_) => { let _ = msg.channel_id.say(&ctx.http, "Invalid amount.").await; return; }
    };
    let target = parts[1].trim().to_string();

    let pool = {
        let data = ctx.data.read().await;
        data.get::<DbPool>().unwrap().clone()
    };

    sqlx::query("UPDATE countries SET balance = balance + $1 WHERE name = $2")
        .bind(amount)
        .bind(&target)
        .execute(&pool)
        .await
        .unwrap();

    let _ = msg.channel_id.say(&ctx.http, format!("✅ Gave {} to {}.", amount, target)).await;
}

async fn cmd_remove(ctx: &Context, msg: &Message, args: &str) {
    if msg.author.id.get() != 1103203454123511878 {
        let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await;
        return;
    }

    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() < 2 { return; }

    let amount: i64 = match parts[0].parse() {
        Ok(n) => n,
        Err(_) => { let _ = msg.channel_id.say(&ctx.http, "Invalid amount.").await; return; }
    };
    let target = parts[1].trim().to_string();

    let pool = {
        let data = ctx.data.read().await;
        data.get::<DbPool>().unwrap().clone()
    };

    sqlx::query("UPDATE countries SET balance = balance - $1 WHERE name = $2")
        .bind(amount)
        .bind(&target)
        .execute(&pool)
        .await
        .unwrap();

    let _ = msg.channel_id.say(&ctx.http, format!("✅ Removed {} from {}.", amount, target)).await;
}

async fn cmd_checkall(ctx: &Context, msg: &Message) {
    if msg.author.id.get() != 1103203454123511878 {
        let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await;
        return;
    }

    let pool = {
        let data = ctx.data.read().await;
        data.get::<DbPool>().unwrap().clone()
    };

    let rows: Vec<(String, i64, i64)> =
        sqlx::query_as("SELECT name, balance, gdp FROM countries ORDER BY balance DESC")
            .fetch_all(&pool)
            .await
            .unwrap();

    let mut out = String::from("🌍 **All Countries:**\n");
    for (name, balance, gdp) in rows {
        out.push_str(&format!("**{}** — Balance: {} | GDP: {}\n", name, balance, gdp));
    }

    let _ = msg.channel_id.say(&ctx.http, out).await;
}

async fn cmd_pay(ctx: &Context, msg: &Message, args: &str) {
    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() < 2 { return; }

    let amount: i64 = match parts[0].parse() {
        Ok(n) => n,
        Err(_) => { let _ = msg.channel_id.say(&ctx.http, "Invalid amount.").await; return; }
    };
    let target = parts[1].trim().to_string();

    let members = load_members();
    let sender_country = members.values()
        .find(|m| m.id.to_ids().contains(&msg.author.id.get()))
        .map(|m| m.country.clone());

    let sender_country = match sender_country {
        Some(c) => c,
        None => { let _ = msg.channel_id.say(&ctx.http, "You don't have a country.").await; return; }
    };

    let valid_target = members.values().any(|m| m.country == target);
    if !valid_target {
        let _ = msg.channel_id.say(&ctx.http, format!("❌ Invalid target: **{}** is not a registered country.", target)).await;
        return;
    }

    let pool = {
        let data = ctx.data.read().await;
        data.get::<DbPool>().unwrap().clone()
    };

    let balance: i64 = sqlx::query_scalar("SELECT balance FROM countries WHERE name = $1")
        .bind(&sender_country)
        .fetch_optional(&pool)
        .await
        .unwrap()
        .unwrap_or(0);

    if amount <= 0 || balance < amount {
        let _ = msg.channel_id.say(&ctx.http, format!("Insufficient funds. Balance: {}", balance)).await;
        return;
    }

    sqlx::query("UPDATE countries SET balance = balance - $1 WHERE name = $2")
        .bind(amount)
        .bind(&sender_country)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("UPDATE countries SET balance = balance + $1 WHERE name = $2")
        .bind(amount)
        .bind(&target)
        .execute(&pool)
        .await
        .unwrap();

    let _ = msg.channel_id.say(&ctx.http, format!("✅ Paid {} to {}.", amount, target)).await;
}

async fn cmd_demote(ctx: &Context, msg: &Message) {
    if msg.author.id.get() != 1103203454123511878 {
        let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await;
        return;
    }

    let guild_id = match msg.guild_id {
        Some(id) => id,
        None => { let _ = msg.channel_id.say(&ctx.http, "Must be used in a server.").await; return; }
    };

    let target_id = match msg.mentions.first() {
        Some(u) => u.id,
        None => { let _ = msg.channel_id.say(&ctx.http, "Usage: `!demote @user`").await; return; }
    };

    let roles = match guild_id.roles(&ctx.http).await {
        Ok(r) => r,
        Err(_) => { let _ = msg.channel_id.say(&ctx.http, "Failed to fetch roles.").await; return; }
    };

    let admin_role = match roles.values().find(|r| r.name.to_lowercase() == "admin") {
        Some(r) => r.id,
        None => { let _ = msg.channel_id.say(&ctx.http, "❌ No role named 'admin' found.").await; return; }
    };

    match guild_id.member(&ctx.http, target_id).await {
        Ok(member) => {
            if !member.roles.contains(&admin_role) {
                let _ = msg.channel_id.say(&ctx.http, "⚠️ That user doesn't have the admin role.").await;
                return;
            }
            match ctx.http.remove_member_role(guild_id, target_id, admin_role, Some("Demoted by owner")).await {
                Ok(_) => { let _ = msg.channel_id.say(&ctx.http, format!("✅ Removed admin role from <@{}>.", target_id)).await; }
                Err(_) => { let _ = msg.channel_id.say(&ctx.http, "❌ Failed to remove role. Check bot permissions.").await; }
            }
        }
        Err(_) => { let _ = msg.channel_id.say(&ctx.http, "❌ Could not find that member.").await; }
    }
}

async fn cmd_save(ctx: &Context, msg: &Message) {
    let blocked: HashSet<u64> = [1236138274855194756u64, 1474908448784121896, 1039098840193716324].into();

    if blocked.contains(&msg.author.id.get()) {
        return;
    }

    let members = load_members();
    let country = members.values()
        .find(|m| m.id.to_ids().contains(&msg.author.id.get()))
        .map(|m| m.country.clone());

    let country = match country {
        Some(c) => c,
        None => { let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await; return; }
    };

    if msg.attachments.is_empty() {
        let _ = msg.channel_id.say(&ctx.http, "No image attached.").await;
        return;
    }

    let save_channel = ChannelId::new(1503636648804483153);

    for attachment in &msg.attachments {
        let builder = serenity::builder::CreateMessage::new()
            .content(format!("📸 Saved by **{}** ({})", country, msg.author.id.get()))
            .add_file(serenity::builder::CreateAttachment::url(&ctx.http, &attachment.url).await.unwrap());

        let _ = save_channel.send_message(&ctx.http, builder).await;
    }

    let _ = msg.channel_id.say(&ctx.http, "✅ Saved.").await;
}

async fn cmd_rng(ctx: &Context, msg: &Message, args: &str) {
    let chance: i64 = match args.trim().parse() {
        Ok(n) => n,
        Err(_) => return,
    };

    if chance < 1 || chance > 100 {
        return;
    }

    let roll = rand::thread_rng().gen_range(1..=100i64);
    
    let response = if roll <= chance {
        format!("✅ Success! (rolled {} against {}%)", roll, chance)
    } else {
        format!("❌ Fail! (rolled {} against {}%)", roll, chance)
    };

    let _ = msg.channel_id.say(&ctx.http, response).await;
}

async fn cmd_ticket(ctx: &Context, msg: &Message) {
    let guild_id = match msg.guild_id {
        Some(id) => id,
        None => return,
    };

    let guild = match guild_id.to_partial_guild(&ctx.http).await {
        Ok(g) => g,
        Err(_) => return,
    };

    let everyone_deny = PermissionOverwrite {
        allow: Permissions::empty(),
        deny: Permissions::VIEW_CHANNEL,
        kind: PermissionOverwriteType::Role(guild.id.everyone_role()),
    };

    let member_allow = PermissionOverwrite {
        allow: Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES,
        deny: Permissions::empty(),
        kind: PermissionOverwriteType::Member(msg.author.id),
    };

    let bot_id = ctx.cache.current_user().id;
    let bot_allow = PermissionOverwrite {
        allow: Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES,
        deny: Permissions::empty(),
        kind: PermissionOverwriteType::Member(bot_id),
    };

    let channel_name = format!("ticket-{}", msg.author.name);
    let builder = serenity::builder::CreateChannel::new(&channel_name)
        .kind(ChannelType::Text)
        .permissions(vec![everyone_deny, member_allow, bot_allow]);

    let channel = match guild.create_channel(&ctx.http, builder).await {
        Ok(c) => c,
        Err(_) => return,
    };

    let _ = channel.say(
        &ctx.http,
        format!("Hey {}, support will be with you shortly. Type `!close` to close the ticket.", msg.author.mention()),
    ).await;

    let _ = msg.channel_id.say(&ctx.http, format!("✅ Ticket created: {}", channel.mention())).await;
}

async fn cmd_close(ctx: &Context, msg: &Message) {
    let channel = match msg.channel_id.to_channel(&ctx.http).await {
        Ok(Channel::Guild(c)) => c,
        _ => return,
    };

    if channel.name.starts_with("ticket-") {
        let _ = msg.channel_id.say(&ctx.http, "Closing ticket...").await;
        let _ = channel.delete(&ctx.http).await;
    } else {
        let _ = msg.channel_id.say(&ctx.http, "This isn't a ticket channel.").await;
    }
}

async fn cmd_weather(ctx: &Context, msg: &Message) {
    let members = load_members();
    let allowed: HashSet<u64> = members.values()
        .flat_map(|m| m.id.to_ids())
        .collect();

    if !allowed.contains(&msg.author.id.get()) {
        let _ = msg.channel_id.say(&ctx.http, "Invalid perms").await;
        return;
    }

    let weather = match rand::thread_rng().gen_range(1..=4u32) {
        1 => "Weather normal",
        2 => "Weather monsoon",
        3 => "Weather winter",
        _ => "Weather drought",
    };

    let _ = msg.channel_id.say(&ctx.http, weather).await;
}

const WATCHED_USER_ID: u64 = 1474908448784121896;

struct ReactionLog;
impl TypeMapKey for ReactionLog {
    type Value = Arc<Mutex<Vec<u64>>>;
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);
    }

    async fn reaction_add(&self, ctx: Context, reaction: Reaction) {
        let user_id = match reaction.user_id {
            Some(id) => id,
            None => return,
        };

        if user_id.get() != WATCHED_USER_ID {
            return;
        }

        let is_israel_flag = match &reaction.emoji {
            ReactionType::Unicode(s) => s == "🇮🇱",
            _ => false,
        };

        if !is_israel_flag {
            let _ = reaction.delete(&ctx.http).await;
            return;
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        let data = ctx.data.read().await;
        let log = data.get::<ReactionLog>().unwrap().clone();
        let mut log = log.lock().await;

        log.retain(|&t| now - t < 300);

        if log.len() >= 3 {
            drop(log);
            let _ = reaction.delete(&ctx.http).await;
        } else {
            log.push(now);
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        let content = msg.content.trim();

        const OWNER_ID: u64 = 1103203454123511878;
        if msg.author.id.get() != OWNER_ID {
            let cfg = load_automod();
            if automod_triggered(&cfg, msg.author.id.get(), content) {
                let _ = msg.delete(&ctx.http).await;

                if let Some(guild_id) = msg.guild_id {
                    let until = {
                        let secs = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs() + 300;
                        let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)
                            .unwrap();
                        dt.to_rfc3339()
                    };
                    let map = serde_json::json!({ "communication_disabled_until": until });
                    let _ = ctx.http.edit_member(guild_id, msg.author.id, &map, None).await;
                }
                return;
            }
        }

        let delete_tracker = load_delete_tracker();
        if is_tracked(&delete_tracker, msg.author.id.get()) {
            let _ = msg.delete(&ctx.http).await;
            return;
        }

        let ignore_list = load_ignore();
        if is_ignored(&ignore_list, msg.author.id.get()) {
            return;
        }

        if let Some(args) = content.strip_prefix("!rng ") {
            cmd_rng(&ctx, &msg, args).await;
        } else if let Some(args) = content.strip_prefix("!pay ") {
            cmd_pay(&ctx, &msg, args).await;
        } else if content == "!checkbalance" {
            cmd_checkbalance(&ctx, &msg).await;
        } else if content == "!ticket" {
            cmd_ticket(&ctx, &msg).await;
        } else if content == "!close" {
            cmd_close(&ctx, &msg).await;
        } else if content == "!weather" {
            cmd_weather(&ctx, &msg).await;
        } else if let Some(args) = content.strip_prefix("!give ") {
            cmd_give(&ctx, &msg, args).await;
        } else if let Some(args) = content.strip_prefix("!remove ") {
            cmd_remove(&ctx, &msg, args).await;
        } else if content == "!checkall" {
            cmd_checkall(&ctx, &msg).await;
        } else if content == "!save" {
            cmd_save(&ctx, &msg).await;
        } else if content.starts_with("!savearsenal") {
            cmd_savearsenal(&ctx, &msg).await;
        } else if let Some(args) = content.strip_prefix("!addgdp ") {
            cmd_addgdp(&ctx, &msg, args).await;
        } else if content == "!stocks" {
            cmd_stocks(&ctx, &msg).await;
        } else if content.starts_with("!demote") {
            cmd_demote(&ctx, &msg).await;
        } else if let Some(args) = content.strip_prefix("!addword ") {
            cmd_addword(&ctx, &msg, args).await;
        } else if let Some(args) = content.strip_prefix("!removeword ") {
            cmd_removeword(&ctx, &msg, args).await;
        } else if let Some(args) = content.strip_prefix("!ignore") {
            let target = args.trim_start();
            cmd_ignore(&ctx, &msg, target).await;
        } else if let Some(args) = content.strip_prefix("!unignore") {
            let target = args.trim_start();
            cmd_unignore(&ctx, &msg, target).await;
        } else if let Some(args) = content.strip_prefix("!deletesentmessages") {
            let target = args.trim_start();
            cmd_deletesentmessages(&ctx, &msg, target).await;
        } else if let Some(args) = content.strip_prefix("!undeletesentmessages") {
            let target = args.trim_start();
            cmd_undeletesentmessages(&ctx, &msg, target).await;
        }
    }
}

#[tokio::main]
async fn main() {
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set");

    let pool = PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to Postgres");

    init_db(&pool).await;

    let pool_for_income = pool.clone();
    tokio::spawn(daily_income(pool_for_income));

    let token = std::env::var("DISCORD_TOKEN")
        .expect("DISCORD_TOKEN must be set");

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::GUILD_MESSAGE_REACTIONS
        | GatewayIntents::GUILD_MEMBERS
        | GatewayIntents::MESSAGE_CONTENT;

    let reaction_log: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));

    let mut client = Client::builder(token, intents)
        .event_handler(Handler)
        .type_map_insert::<DbPool>(pool)
        .type_map_insert::<ReactionLog>(reaction_log)
        .await
        .expect("Failed to create client");
    
    let http_for_stocks = client.http.clone();
    tokio::spawn(daily_stocks(http_for_stocks));

    if let Err(e) = client.start().await {
        eprintln!("Client error: {:?}", e);
    }
}