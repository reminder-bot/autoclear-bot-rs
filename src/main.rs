#[macro_use] extern crate serenity;
#[macro_use] extern crate mysql;

extern crate dotenv;
extern crate typemap;
extern crate reqwest;

use std::env;
use serenity::prelude::{Context, EventHandler};
use serenity::model::gateway::{Game, Ready};
use serenity::model::event::ChannelPinsUpdateEvent;
use dotenv::dotenv;
use typemap::Key;
use serenity::model::channel::*;
use std::collections::HashMap;


struct Globals;

impl Key for Globals {
    type Value = mysql::Pool;
}


struct Handler;

impl EventHandler for Handler {
    fn guild_create(&self, _context: Context, _guild: serenity::model::guild::Guild, _new: bool) {
        let guild_count = {
            let cache = serenity::CACHE.read();
            cache.all_guilds().len()
        };

        let c = reqwest::Client::new();
        let mut m = HashMap::new();
        m.insert("server_count", guild_count);

        let _ = c.post("https://discordbots.org/api/bots/stats").header("Authorization", env::var("DBL_TOKEN").unwrap()).header("Content-Type", "application/json").json(&m).send().unwrap();
    }

    fn guild_delete(&self, _context: Context, _guild: serenity::model::guild::PartialGuild, _full: Option<std::sync::Arc<serenity::prelude::RwLock<serenity::model::guild::Guild>>>) {
        let guild_count = {
            let cache = serenity::CACHE.read();
            cache.all_guilds().len()
        };

        let c = reqwest::Client::new();
        let mut m = HashMap::new();
        m.insert("server_count", guild_count);

        c.post("https://discordbots.org/api/bots/stats").header("Authorization", env::var("DBL_TOKEN").unwrap()).header("Content-Type", "application/json").json(&m).send().unwrap();
    }

    fn ready(&self, context: Context, _: Ready) {
        println!("Bot online!");

        context.set_game(Game::playing("@Automaid help"));
    }

    fn channel_pins_update(&self, context: Context, pin: ChannelPinsUpdateEvent) {

        let data = context.data.lock();
        let mysql = data.get::<Globals>().unwrap();

        for pin in pin.channel_id.pins().unwrap() {
            let id = pin.id;

            mysql.prep_exec(r#"DELETE FROM deletes WHERE message = :id"#, params!{"id" => id.as_u64()}).unwrap();
        }
    }

    fn message(&self, ctx: Context, message: Message) {
        let c = message.channel_id;
        let user = match message.webhook_id {
            Some(w) => w.to_webhook().unwrap().user.unwrap(),

            None => message.author,
        };

        let m = user.id.as_u64();


        let data = ctx.data.lock();
        let mysql = data.get::<Globals>().unwrap();

        let mut res = mysql.prep_exec(r#"SELECT MIN(timeout) FROM channels WHERE channel = :id AND (user is null OR user = :u)"#, params!{"id" => c.as_u64(), "u" => m}).unwrap();

        match res.next() {
            Some(r) => {
                let timeout = mysql::from_row::<Option<u32>>(r.unwrap());

                match timeout {
                    Some(t) => {
                        let msg = message.id;

                        mysql.prep_exec(r#"INSERT INTO deletes (channel, message, `time`) VALUES (:id, :msg, ADDDATE(NOW(), INTERVAL :t SECOND))"#, params!{"id" => c.as_u64(), "msg" => msg.as_u64(), "t" => t}).unwrap();
                    },

                    None =>
                        return (),
                }

            },

            None =>
                return (),
        }
    }
}


fn main() {
    dotenv().ok();

    let token = env::var("DISCORD_TOKEN").expect("token");
    let sql_url = env::var("SQL_URL").expect("sql url");

    let mut client = serenity::client::Client::new(&token, Handler).unwrap();
    client.with_framework(serenity::framework::standard::StandardFramework::new()
        .configure(|c| c
            .prefix("autoclear ")
            .on_mention(true)
        )

        .cmd("help", help)
        .cmd("invite", info)
        .cmd("info", info)
        .cmd("start", autoclear)
        .cmd("stop", cancel_clear)
        .cmd("rules", rules)
        .cmd("clear", clear)
        .cmd("purge", purge)
    );

    let my = mysql::Pool::new(sql_url).unwrap();

    {
        let mut data = client.data.lock();
        data.insert::<Globals>(my);
    }

    if let Err(e) = client.start() {
        println!("An error occured: {:?}", e);
    }
}


command!(autoclear(context, message, args) {
    match message.member().unwrap().permissions() {
        Ok(p) => {
            if !p.manage_guild() {
                let _ = message.reply("You must be a guild manager to perform this command");
            }
            else {
                let mut timeout = 10;

                for arg in args.iter::<String>() {
                    let a = arg.unwrap();

                    if is_numeric(&a) {
                        timeout = a.parse().unwrap();
                        break;
                    }
                }

                let to_send = args.rest();

                let msg = if to_send.is_empty() { None } else { Some(to_send) };

                let c = message.channel_id;

                let data = context.data.lock();
                let mysql = data.get::<Globals>().unwrap();

                let mn = &message.mentions;

                if mn.len() == 0 {
                    mysql.prep_exec(r#"DELETE FROM channels WHERE channel = :c AND user IS NULL"#, params!{"c" => c.as_u64()}).unwrap();
                    mysql.prep_exec(r#"INSERT INTO channels (channel, timeout, message) VALUES (:c, :t, :m)"#, params!{"c" => c.as_u64(), "t" => timeout, "m" => msg}).unwrap();

                    let _ = message.reply("Autoclearing channel.");
                }
                else {
                    for mention in mn {
                        mysql.prep_exec(r#"DELETE FROM channels WHERE channel = :c AND user = :u"#, params!{"c" => c.as_u64(), "u" => mention.id.as_u64()}).unwrap();
                        mysql.prep_exec(r#"INSERT INTO channels (channel, user, timeout, message) VALUES (:c, :u, :t, :m)"#, params!{"c" => c.as_u64(), "u" => mention.id.as_u64(), "t" => timeout, "m" => msg}).unwrap();
                    }
                    let _ = message.reply(&format!("Autoclearing {} users.", mn.len()));
                }
            }
        },

        Err(_) => {

        },
    }
});


command!(cancel_clear(context, message) {
    match message.member().unwrap().permissions() {
        Ok(p) => {
            if !p.manage_guild() {
                let _ = message.reply("You must be a guild manager to perform this command");
            }
            else {
                let c = message.channel_id;

                let data = context.data.lock();
                let mysql = data.get::<Globals>().unwrap();

                let mn = &message.mentions;

                if mn.len() == 0 {
                    mysql.prep_exec(r#"DELETE FROM channels WHERE channel = :c AND user IS NULL"#, params!{"c" => c.as_u64()}).unwrap();

                    let _ = message.reply("Global autoclear cancelled on this channel.");
                }
                else {
                    for mention in mn {
                        mysql.prep_exec(r#"DELETE FROM channels WHERE channel = :c AND user = :u"#, params!{"c" => c.as_u64(), "u" => mention.id.as_u64()}).unwrap();
                    }
                    let _ = message.reply(&format!("Autoclear cancelled on {} users.", mn.len()));
                }
            }
        },

        Err(_) => {

        },
    }
});


command!(rules(context, message) {
    match message.member().unwrap().permissions() {
        Ok(p) => {
            if !p.manage_guild() {
                let _ = message.reply("You must be a guild manager to perform this command");
            }
            else {
                let c = message.channel_id;

                let data = context.data.lock();
                let mysql = data.get::<Globals>().unwrap();

                let res = mysql.prep_exec(r#"SELECT user, timeout FROM channels WHERE channel = :c"#, params!{"c" => c.as_u64()}).unwrap();

                let mut out: Vec<String> = vec![];

                for row in res {
                    let (u_id, t) = mysql::from_row::<(Option<u64>, u32)>(row.unwrap());
                    match u_id {
                        Some(u) => {
                            out.push(format!("**<@{}>**: {}s", u, t));
                        },

                        None => {
                            out.insert(0, format!("**GLOBAL**: {}s", t));
                        },
                    }
                }

                let _ = c.send_message(|m| m
                    .embed(|e| e
                        .title("Rules")
                        .description(out.join("\n"))
                    )
                );
            }
        },

        Err(_) => {

        },
    }
});


command!(clear(_context, message) {
    match message.member().unwrap().permissions() {
        Ok(p) => {
            if !p.manage_guild() {
                let _ = message.reply("You must be a guild manager to perform this command");
            }
            else {
                let messages = message.channel_id.messages(|m| m.limit(100)).unwrap();
                let tag = match message.mentions.get(0){
                    Some(o) => o,

                    None => {
                        let _ = message.reply("Please mention a user to clear messages of.");
                        return Ok(())
                    }
                };

                let mut deletes = vec![];

                for m in messages {
                    if m.author.id == tag.id {
                        deletes.push(m.id);
                    }
                }

                let r = message.channel_id.delete_messages(deletes);

                match r {
                    Ok(_) => {
                        return Ok(())
                    },

                    Err(_) => {
                        let _ = message.channel_id.send_message(|m|
                            m.content("An error occured during deleting messages. Maybe the user hasn't sent messages, or the messages are +14d old?"));
                    },
                }
            }
        },

        Err(_) => {

        },
    }
});


command!(purge(_context, message, args) {
    match message.member().unwrap().permissions() {
        Ok(p) => {
            if !p.manage_guild() {
                let _ = message.reply("You must be a guild manager to perform this command");
            }
            else {
                match args.single::<u64>() {
                    Ok(num) => {
                        if num <= 100 {
                            let messages = message.channel_id.messages(|m| m.limit(num)).unwrap();
                            let r = message.channel_id.delete_messages(messages);

                            match r {
                                Ok(_) => {
                                    return Ok(())
                                },

                                Err(_) => {
                                    let _ = message.channel_id.send_message(|m|
                                        m.content("An error occured during deleting messages. Messages may be +14d old."));
                                },
                            }
                        }
                        else {
                            let _ = message.reply("Please provide a number less than 100 of messages to clear.");
                        }
                    },

                    Err(_) => {
                        let _ = message.reply("Please provide a number less than 100 of messages to clear.");
                    }
                }
            }
        },

        Err(_) => {

        },
    }
});


fn is_numeric(s: &String) -> bool {
    let m = s.matches(char::is_numeric);

    if m.into_iter().count() == s.len() {
        return true
    }
    false
}

command!(help(_context, message) {
    let _ = message.channel_id.send_message(|m| {
        m.embed(|e| {
            e.title("Help")
            .description("`autoclear start` - Start autoclearing the current channel. Accepts arguments:
\t* User mentions (users the clear applies to- if no mentions, will do all users)
\t* Duration (time in seconds that messages should remain for- defaults to 10s)
\t* Message (optional, message to send when a message is deleted)

\tE.g `autoclear start @JellyWX#2946 5`

`autoclear clear` - Delete message history of specific users. Accepts arguments:
\t* User mention (user to clear history of)

`autoclear purge` - Delete message history. Accepts arguments:
\t* Limit (number of messages to delete)

`autoclear rules` - Check the autoclear rules for specified channels. Accepts arguments:
\t* Channel mention (channel to view rules of- defaults to current)

`autoclear stop` - Cancel autoclearing on current channel. Accepts arguments:
\t* User mentions (users to cancel autoclearing for- if no mentions, will do all users)")
        })
    });
});


command!(info(_context, message) {
    let _ = message.channel_id.send_message(|m| {
        m.embed(|e| {
            e.title("Info")
            .description("
Invite me: https://discordapp.com/oauth2/authorize?client_id=488060245739044896&scope=bot&permissions=93184

Automaid is a part of the Fusion Network:
https://discordbots.org/servers/366542432671760396

Do `~help` for more.

Logo credit: **Font Awesome 2018 CC-BY 4.0**
            ")
        })
    });
});
