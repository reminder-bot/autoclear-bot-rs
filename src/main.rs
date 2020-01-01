extern crate serenity;
#[macro_use] extern crate mysql;

extern crate dotenv;
extern crate typemap;

use std::env;
use serenity::client::Client;
use serenity::prelude::{Context, EventHandler};
use serenity::model::{
    event::ChannelPinsUpdateEvent,
    gateway::{
        Activity,
        Ready,
    },
    channel::{
        Channel,
        Message,
    },
    permissions::Permissions,
};
use serenity::framework::standard::{
    Args,
    StandardFramework,
    CommandResult,
    macros::{
        command,
        group,
    },
};
use dotenv::dotenv;
use typemap::Key;

group!({
    name: "general",
    options: {
        required_permissions: [
            MANAGE_MESSAGES,
        ],
    },
    commands: [
        help,
        info,
        autoclear,
        cancel_clear,
        rules,
    ],
});

struct Globals;

impl Key for Globals {
    type Value = mysql::Pool;
}

struct Handler;

impl EventHandler for Handler {
    fn ready(&self, context: Context, _: Ready) {
        println!("Bot online now");

        context.set_activity(Activity::playing("@Automaid help"));
    }

    fn channel_pins_update(&self, context: Context, pin: ChannelPinsUpdateEvent) {

        let data = context.data.read();
        let mysql = data.get::<Globals>().unwrap();

        for pin in pin.channel_id.pins(&context).unwrap() {
            let id = pin.id;

            mysql.prep_exec(r#"DELETE FROM deletes WHERE message = :id"#, params!{"id" => id.as_u64()}).unwrap();
        }
    }

    fn message(&self, ctx: Context, message: Message) {

        let c = message.channel_id;

        if let Ok(Channel::Guild(guild_channel)) = c.to_channel(&ctx) {

            let current_user_id = ctx.cache.read().user.id;
            let guild_channel = guild_channel.read();

            let permissions = guild_channel.permissions_for_user(&ctx, current_user_id).unwrap();

            if permissions.contains(Permissions::MANAGE_WEBHOOKS & Permissions::MANAGE_MESSAGES) {

                let user = match message.webhook_id {
                    Some(w) => w.to_webhook(&ctx).unwrap().user.unwrap(),

                    None => message.author,
                };

                let m = user.id.as_u64();

                let data = ctx.data.read();
                let mysql = data.get::<Globals>().unwrap();

                let mut res = mysql.prep_exec(r#"SELECT timeout, message FROM channels WHERE channel = :id AND (user is null OR user = :u) AND timeout = (SELECT MIN(timeout) FROM channels WHERE channel = :id AND (user is null OR user = :u))"#, params!{"id" => c.as_u64(), "u" => m}).unwrap();

                match res.next() {
                    Some(r) => {
                        let (timeout, mut text) = mysql::from_row::<(Option<u32>, Option<String>)>(r.unwrap());

                        match timeout {
                            Some(t) => {
                                let msg = message.id;

                                if user.bot {
                                    text = None;
                                }

                                mysql.prep_exec(r#"INSERT INTO deletes (channel, message, `time`, to_send) VALUES (:id, :msg, ADDDATE(NOW(), INTERVAL :t SECOND), :text)"#, params!{"id" => c.as_u64(), "msg" => msg.as_u64(), "t" => t, "text" => text}).unwrap();
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
    }
}


fn main() {
    dotenv().ok();

    let token = env::var("DISCORD_TOKEN").expect("token");
    let sql_url = env::var("SQL_URL").expect("sql url");

    let mut client = Client::new(&token, Handler).expect("Failed to create client");

    let user_id;

    {
        let http_client = serenity::http::raw::Http::new_with_token(&format!("Bot {}", &token));

        match http_client.get_current_user() {
            Ok(user) => {
                user_id = user.id;
            },

            Err(e) => {
                println!("{:?}", e);
                panic!("Failed to get ID of current user. Is token valid?");
            }
        }
    }

    client.with_framework(
        StandardFramework::new()
            .configure(|c| c
                .prefix("autoclear ")
                .allow_dm(false)
                .ignore_bots(true)
                .ignore_webhooks(true)
                .on_mention(Some(user_id))
            )
            .group(&GENERAL_GROUP)
    );

    let my = mysql::Pool::new(sql_url).unwrap();

    {
        let mut data = client.data.write();
        data.insert::<Globals>(my);
    }

    if let Err(e) = client.start_autosharded() {
        println!("An error occured: {:?}", e);
    }
}

#[command("start")]
fn autoclear(context: &mut Context, message: &Message, mut args: Args) -> CommandResult {
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

    let data = context.data.read();
    let mysql = data.get::<Globals>().unwrap();

    let mn = &message.mentions;

    if mn.len() == 0 {
        mysql.prep_exec(r#"DELETE FROM channels WHERE channel = :c AND user IS NULL"#, params!{"c" => c.as_u64()}).unwrap();
        mysql.prep_exec(r#"INSERT INTO channels (channel, timeout, message) VALUES (:c, :t, :m)"#, params!{"c" => c.as_u64(), "t" => timeout, "m" => msg}).unwrap();

        let _ = message.reply(&context, "Autoclearing channel.");
    }
    else {
        for mention in mn {
            mysql.prep_exec(r#"DELETE FROM channels WHERE channel = :c AND user = :u"#, params!{"c" => c.as_u64(), "u" => mention.id.as_u64()}).unwrap();
            mysql.prep_exec(r#"INSERT INTO channels (channel, user, timeout, message) VALUES (:c, :u, :t, :m)"#, params!{"c" => c.as_u64(), "u" => mention.id.as_u64(), "t" => timeout, "m" => msg}).unwrap();
        }
        let _ = message.reply(&context, &format!("Autoclearing {} users.", mn.len()));
    }

    Ok(())
}


#[command("stop")]
fn cancel_clear(context: &mut Context, message: &Message) -> CommandResult {
    let c = message.channel_id;

    let data = context.data.read();
    let mysql = data.get::<Globals>().unwrap();

    let mn = &message.mentions;

    if mn.len() == 0 {
        mysql.prep_exec(r#"DELETE FROM channels WHERE channel = :c AND user IS NULL"#, params!{"c" => c.as_u64()}).unwrap();

        let _ = message.reply(&context, "Global autoclear cancelled on this channel.");
    }
    else {
        for mention in mn {
            mysql.prep_exec(r#"DELETE FROM channels WHERE channel = :c AND user = :u"#, params!{"c" => c.as_u64(), "u" => mention.id.as_u64()}).unwrap();
        }
        let _ = message.reply(&context, &format!("Autoclear cancelled on {} users.", mn.len()));
    }

    Ok(())
}


#[command("rules")]
fn rules(context: &mut Context, message: &Message) -> CommandResult {
    let c = message.channel_id;

    let mut out: Vec<String> = vec![];

    {
        let data = context.data.read();
        let mysql = data.get::<Globals>().unwrap();

        let res = mysql.prep_exec(r#"SELECT user, timeout FROM channels WHERE channel = :c"#, params!{"c" => c.as_u64()}).unwrap();

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
    }

    let _ = c.send_message(context, |m| m
        .embed(|e| e
            .title("Rules")
            .description(out.join("\n"))
        )
    );

    Ok(())
}


fn is_numeric(s: &String) -> bool {
    let m = s.matches(char::is_numeric);

    if m.into_iter().count() == s.len() {
        return true
    }
    false
}


#[command]
fn help(context: &mut Context, message: &Message) -> CommandResult {
    let _ = message.channel_id.send_message(context, |m| {
        m.embed(|e| {
            e.title("Help")
            .description("`autoclear start` - Start autoclearing the current channel. Accepts arguments:
\t* User mentions (users the clear applies to- if no mentions, will do all users)
\t* Duration (time in seconds that messages should remain for- defaults to 10s)
\t* Message (optional, message to send when a message is deleted)

\tE.g `autoclear start @JellyWX#2946 5`

`autoclear rules` - Check the autoclear rules for specified channels. Accepts arguments:
\t* Channel mention (channel to view rules of- defaults to current)

`autoclear stop` - Cancel autoclearing on current channel. Accepts arguments:
\t* User mentions (users to cancel autoclearing for- if no mentions, will do all users)")
        })
    });

    Ok(())
}


#[command]
fn info(context: &mut Context, message: &Message) -> CommandResult {
    let _ = message.channel_id.send_message(context, |m| {
        m.embed(|e| {
            e.title("Info")
            .description("
Invite me: https://discordapp.com/oauth2/authorize?client_id=488060245739044896&scope=bot&permissions=93184

Join the Discord server: https://discord.jellywx.com/

Do `autoclear help` for more.

Logo credit: **Font Awesome 2018 CC-BY 4.0**
            ")
        })
    });

    Ok(())
}
