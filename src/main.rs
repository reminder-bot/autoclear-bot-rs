use serenity::{
    client::{
        bridge::{
            gateway::GatewayIntents,
            voice::ClientVoiceManager,
        },
        Client, Context,
    },
    framework::standard::{
        Args, CommandResult, CheckResult, DispatchError, StandardFramework, Reason,
        macros::{
            command, group, check, hook,
        }
    },
    model::{
        channel::{
            Channel,
            Message,
        },
        event::ChannelPinsUpdateEvent,
        id::{
            GuildId,
            RoleId,
            UserId,
        },
        voice::VoiceState,
    },
    prelude::{
        Mutex as SerenityMutex,
        *
    },
    voice::Handler as VoiceHandler,
};

use sqlx::{
    Pool,
    mysql::{
        MySqlPool,
        MySqlConnection,
    }
};

use dotenv::dotenv;

use std::env;

#[group]
#[commands(help, info, autoclear, cancel_clear, rules)]
#[checks(permission_check)]
struct General;

async fn permission_check(ctx: &Context, msg: &&Message) -> CheckResult {
    if let Some(guild_id) = msg.guild_id {
        if let Ok(member) = guild_id.member(ctx.clone(), msg.author.id).await {
            if let Ok(perms) = member.permissions(ctx).await {
                if perms.manage_messages() || perms.manage_guild() || perms.administrator() {
                    return CheckResult::Success
                }
            }
        }
    }

    CheckResult::Failure(Reason::User(String::from("User needs `Manage Guild` permission")))
}


struct SQLPool;

impl TypeMapKey for SQLPool {
    type Value = Pool<MySqlConnection>;
}

struct Handler;

#[serenity:async_trait]
impl EventHandler for Handler {
    async fn channel_pins_update(&self, context: Context, pin: ChannelPinsUpdateEvent) {

        let data = context.data.read();
        let mysql = data.get::<SQLPool>().unwrap();

        for pin in pin.channel_id.pins(&context).unwrap() {
            let id = pin.id;

            mysql.prep_exec(r#"DELETE FROM deletes WHERE message = :id"#, params!{"id" => id.as_u64()}).unwrap();
        }
    }

    async fn message(&self, ctx: Context, message: Message) {

        let c = message.channel_id;

        if let Ok(Channel::Guild(guild_channel)) = c.to_channel(&ctx).await {

            let current_user_id = ctx.cache.read().user.id;

            let permissions = guild_channel.permissions_for_user(&ctx, current_user_id).unwrap();

            if permissions.contains(Permissions::MANAGE_WEBHOOKS) && permissions.contains(Permissions::MANAGE_MESSAGES) {

                dbg!("Permissions valid");

                let user = match message.webhook_id {
                    Some(w) => w.to_webhook(&ctx).unwrap().user.unwrap(),

                    None => message.author,
                };

                let m = user.id.as_u64();

                let data = ctx.data.read();
                let mysql = data.get::<SQLPool>().unwrap();

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

#[tokio::main]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let token = env::var("DISCORD_TOKEN").expect("token");

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


    let framework = StandardFramework::new()
        .configure(|c| c
            .dynamic_prefix(|ctx, msg| Box::pin(async move {
                let pool = ctx.data.read().await
                    .get::<SQLPool>().cloned().expect("Could not get SQLPool from data");

                let guild = match msg.guild(&ctx.cache).await {
                    Some(guild) => guild,

                    None => {
                        return Some(String::from("?"));
                    }
                };

                match GuildData::get_from_id(*msg.guild_id.unwrap().as_u64(), pool.clone()).await {
                    Some(mut guild_data) => {
                        let name = Some(guild.name);

                        if guild_data.name != name {
                            guild_data.name = name;
                            guild_data.commit(pool).await.unwrap();
                        }
                        Some(guild_data.prefix)
                    },

                    None => {
                        GuildData::create_from_guild(guild, pool).await.unwrap();
                        Some(String::from("?"))
                    }
                }
            }))
            .allow_dm(false)
            .ignore_bots(true)
            .ignore_webhooks(true)
            .on_mention(user_id)
        )
        .group(&GENERAL_GROUPS)
        .after(log_errors)
        .on_dispatch_error(dispatch_error_hook);

    let mut client = Client::new(&env::var("DISCORD_TOKEN").expect("Missing token from environment"))
        .intents(GatewayIntents::GUILD_VOICE_STATES | GatewayIntents::GUILD_MESSAGES | GatewayIntents::GUILDS)
        .framework(framework)
        .event_handler(Handler)
        .await.expect("Error occurred creating client");

    {
        let pool = MySqlPool::new(&env::var("DATABASE_URL").expect("No database URL provided")).await.unwrap();

        let mut data = client.data.write().await;
        data.insert::<SQLPool>(pool);

    }

    client.start_autosharded().await?;

    Ok(())
}

#[command("start")]
async fn autoclear(context: &Context, message: &Message, mut args: Args) -> CommandResult {
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

    let channel_id = message.channel_id;

    let pool = ctx.data.read().await
        .get::<SQLPool>().cloned().expect("Could not get SQLPool from data");

    let mentions = &message.mentions;

    if mentions.len() == 0 {
        sqlx::query!(
            "
DELETE FROM channels WHERE channel = ? AND user IS NULL;
INSERT INTO channels (channel, timeout, message) VALUES (?, ?, ?);
            ",
            c.as_u64(), c.as_u64(),
            timeout, msg
        )
            .execute(&pool)
            .await?;

        let _ = message.reply(&context, "Autoclearing channel.");
    }
    else {
        for mention in mentions {
            sqlx::query!(
                "
DELETE FROM channels WHERE channel = ? AND user = ?;
INSERT INTO channels (channel, user, timeout, message) VALUES (?, ?, ?, ?);
                ",
                c.as_u64(), mention.id.as_u64(),
                c.as_u64(), mention.id.as_u64(), timeout, msg
            )
            .execute(&pool)
            .await?;
        }

        let _ = message.reply(&context, &format!("Autoclearing {} users.", mentions.len()));
    }

    Ok(())
}


#[command("stop")]
async fn cancel_clear(context: &Context, message: &Message) -> CommandResult {
    let c = message.channel_id;

    let data = context.data.read();
    let mysql = data.get::<SQLPool>().unwrap();

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
fn rules(context: &Context, message: &Message) -> CommandResult {
    let c = message.channel_id;

    let mut out: Vec<String> = vec![];

    {
        let data = context.data.read();
        let mysql = data.get::<SQLPool>().unwrap();

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
async fn help(context: &Context, message: &Message) -> CommandResult {
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
async fn info(context: &Context, message: &Message) -> CommandResult {
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
