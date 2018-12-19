#[macro_use] extern crate serenity;
#[macro_use] extern crate mysql;

extern crate dotenv;
extern crate typemap;
extern crate reqwest;

use std::env;
use serenity::prelude::{Context, EventHandler};
use serenity::model::gateway::{Game, Ready};
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

    fn message(&self, ctx: Context, message: Message) {
        let c = message.channel_id;
        let m = message.author.id.as_u64();

        let data = ctx.data.lock();
        let mysql = data.get::<Globals>().unwrap();

        let mut res = mysql.prep_exec(r#"SELECT MIN(timeout) FROM channels WHERE channel = :id and user is null or user = :u"#, params!{"id" => c.as_u64(), "u" => m}).unwrap();

        match res.next() {
            Some(r) => {
                let timeout = mysql::from_row::<u32>(r.unwrap());
                let msg = message.id;

                mysql.prep_exec(r#"INSERT INTO deletes (channel, message, `time`) VALUES (:id, :msg, ADDDATE(NOW(), INTERVAL :t SECOND))"#, params!{"id" => c.as_u64(), "msg" => msg.as_u64(), "t" => timeout});
            },

            None => return (),
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
                for arg in args.iter::<String>() {

                }
            }
        },

        Err(_) => {

        },
    }
});


command!(help(_context, message) {
    let _ = message.channel_id.send_message(|m| {
        m.embed(|e| {
            e.title("Help")
            .description("`autoclear start` - Start autoclearing the current channel. Accepts arguments:
\t* User mentions (users the clear applies to- if no mentions, will do all users)
\t* Duration (time in seconds that messages should remain for- defaults to 10s)

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
Invite me: https://discordapp.com/oauth2/authorize?client_id=474240839900725249&scope=bot&permissions=93264

Suggestion Bot is a part of the Fusion Network:
https://discordbots.org/servers/366542432671760396

Do `~help` for more.
            ")
        })
    });
});
