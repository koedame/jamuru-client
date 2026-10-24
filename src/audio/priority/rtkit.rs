//! RealtimeKit (rtkit): the system service that hands out real-time priority to ordinary
//! users on a desktop, over the system D-Bus.

use std::time::Duration;

use zbus::block_on;
use zbus::connection::Builder;
use zbus::proxy::{Builder as ProxyBuilder, CacheProperties};
use zbus::{Connection, Proxy};

const DESTINATION: &str = "org.freedesktop.RealtimeKit1";
const PATH: &str = "/org/freedesktop/RealtimeKit1";

/// How long a call to rtkit may take. It answers in a millisecond or two; polkit, which it asks,
/// is the slow part.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) struct RealtimeKit {
    proxy: Proxy<'static>,
}

impl RealtimeKit {
    /// # Errors
    /// Why there is no rtkit to ask: no system bus, or nothing on it by that name.
    pub(super) fn connect() -> Result<Self, String> {
        let connection = block_on(Builder::system().method_timeout(CALL_TIMEOUT).build())
            .map_err(|e| format!("no system bus: {e}"))?;
        Self::on(&connection)
    }

    fn on(connection: &Connection) -> Result<Self, String> {
        // rtkit answers `Get` but not `GetAll`, which a proxy that caches properties sends first
        let proxy = block_on(
            ProxyBuilder::new(connection)
                .destination(DESTINATION)
                .path(PATH)
                .interface(DESTINATION)
                .cache_properties(CacheProperties::No)
                .build(),
        )
        .map_err(|e| e.to_string())?;
        Ok(Self { proxy })
    }

    /// Makes the thread `tid` `SCHED_RR`, at `wanted` or the highest priority rtkit allows.
    /// Returns the priority given.
    ///
    /// # Errors
    /// Why rtkit did not: it is not running, or turned the request down.
    pub(super) fn make_thread_realtime(
        &self,
        tid: libc::pid_t,
        wanted: i32,
    ) -> Result<i32, String> {
        let highest: i32 = self
            .property("MaxRealtimePriority")
            .map_err(|e| format!("no rtkit: {e}"))?;
        let priority = wanted.min(highest);
        if priority < 1 {
            return Err(format!("rtkit allows no real-time priority ({highest})"));
        }
        let rttime_max: i64 = self.property("RTTimeUSecMax")?;
        limit_real_time_cpu(rttime_max)?;
        block_on(
            self.proxy
                .call::<_, _, ()>("MakeThreadRealtime", &(tid as u64, priority as u32)),
        )
        .map_err(|e| e.to_string())?;
        Ok(priority)
    }

    /// Gives the thread `tid` the lowest nice value rtkit allows. Returns it.
    ///
    /// # Errors
    /// Why rtkit did not.
    pub(super) fn make_thread_high_priority(&self, tid: libc::pid_t) -> Result<i32, String> {
        let nice: i32 = self.property("MinNiceLevel")?;
        block_on(
            self.proxy
                .call::<_, _, ()>("MakeThreadHighPriority", &(tid as u64, nice)),
        )
        .map_err(|e| e.to_string())?;
        Ok(nice)
    }

    fn property<T>(&self, name: &str) -> Result<T, String>
    where
        T: serde::de::DeserializeOwned + zbus::Type,
    {
        block_on(self.proxy.get_property(name)).map_err(|e| e.to_string())
    }
}

/// rtkit gives real-time priority only to a process that cannot hold the CPU for long with it:
/// `RLIMIT_RTTIME`, the CPU time a thread may use in real-time without blocking, must be no
/// more than `max_usec`, soft and hard. A thread that goes over it is sent `SIGXCPU`.
///
/// The limit applies to the whole process from here on, which the audio threads live with: they
/// block on a device or a socket every period.
fn limit_real_time_cpu(max_usec: i64) -> Result<(), String> {
    let max = max_usec as libc::rlim_t;
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: reads the limit into a struct that lives through the call
    if unsafe { libc::getrlimit(libc::RLIMIT_RTTIME, &mut limit) } != 0 {
        return Err(format!("getrlimit: {}", std::io::Error::last_os_error()));
    }
    if limit.rlim_cur <= max && limit.rlim_max <= max {
        return Ok(());
    }
    limit.rlim_cur = limit.rlim_cur.min(max);
    limit.rlim_max = limit.rlim_max.min(max);
    // SAFETY: sets a limit from a struct that lives through the call
    if unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &limit) } != 0 {
        return Err(format!("setrlimit: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};
    use zbus::Variant;

    fn cpu_time_limit() -> libc::rlimit {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: reads the limit into a struct that lives through the call
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_RTTIME, &mut limit) },
            0
        );
        limit
    }

    #[test]
    fn when_the_cpu_time_limit_is_above_what_rtkit_allows_it_is_brought_down_to_it() {
        limit_real_time_cpu(200_000).unwrap();

        let limit = cpu_time_limit();
        assert!(limit.rlim_cur <= 200_000, "soft limit {}", limit.rlim_cur);
        assert!(limit.rlim_max <= 200_000, "hard limit {}", limit.rlim_max);
    }

    #[test]
    fn when_the_cpu_time_limit_is_already_lower_it_is_left_as_it_is() {
        limit_real_time_cpu(100_000).unwrap();

        limit_real_time_cpu(200_000).unwrap();

        assert_eq!(cpu_time_limit().rlim_cur, 100_000);
    }

    /// What the stand-in for rtkit was asked: the method and its arguments
    type Calls = Arc<Mutex<Vec<(String, u64, i64)>>>;

    /// Answers as rtkit does: `Get` for its three properties and the two thread calls. Anything
    /// else, `GetAll` included, is an unknown method, as it is for the real one.
    ///
    /// `messages` is made by the caller before anything can call: a stream only sees what
    /// arrives after it exists, so one made on this thread could miss a call that came first.
    fn answer(
        connection: &Connection,
        mut messages: zbus::MessageStream,
        calls: &Calls,
        highest: i32,
    ) {
        while let Some(message) = block_on(messages.next()) {
            let Ok(message) = message else { break };
            let header = message.header();
            if header.message_type() != zbus::message::Type::MethodCall {
                continue;
            }
            let member = header.member().map(|m| m.to_string()).unwrap_or_default();
            let body = message.body();
            let reply = block_on(async {
                match member.as_str() {
                    "Get" => {
                        let (_interface, name): (String, String) = body.deserialize().unwrap();
                        match name.as_str() {
                            "MaxRealtimePriority" => {
                                connection.reply(&header, &Variant::from(highest)).await
                            }
                            "RTTimeUSecMax" => {
                                connection.reply(&header, &Variant::from(200_000_i64)).await
                            }
                            "MinNiceLevel" => {
                                connection.reply(&header, &Variant::from(-15_i32)).await
                            }
                            _ => {
                                connection
                                    .reply_error(
                                        &header,
                                        "org.freedesktop.DBus.Error.UnknownProperty",
                                        &name,
                                    )
                                    .await
                            }
                        }
                    }
                    "MakeThreadRealtime" => {
                        let (thread, priority): (u64, u32) = body.deserialize().unwrap();
                        calls
                            .lock()
                            .unwrap()
                            .push((member.clone(), thread, priority.into()));
                        connection.reply(&header, &()).await
                    }
                    "MakeThreadHighPriority" => {
                        let (thread, nice): (u64, i32) = body.deserialize().unwrap();
                        calls
                            .lock()
                            .unwrap()
                            .push((member.clone(), thread, nice.into()));
                        connection.reply(&header, &()).await
                    }
                    _ => {
                        connection
                            .reply_error(
                                &header,
                                "org.freedesktop.DBus.Error.UnknownMethod",
                                &format!("Method \"{member}\" doesn't exist"),
                            )
                            .await
                    }
                }
            });
            reply.unwrap();
        }
    }

    /// A bus of its own, with the stand-in on it, and a client connection to it
    struct Bus {
        daemon: Child,
        _service: Connection,
        client: RealtimeKit,
        calls: Calls,
        /// Last, so the daemon is gone before the next test may count children
        _turn: std::sync::MutexGuard<'static, ()>,
    }

    impl Bus {
        fn start(highest: i32) -> Self {
            let turn = crate::perf::child_process_turn();
            let mut daemon = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address"])
                .stdout(Stdio::piped())
                .spawn()
                .expect("dbus-daemon is needed to test the rtkit calls");
            let mut address = String::new();
            BufReader::new(daemon.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            let address = address.trim().to_string();
            let calls = Calls::default();
            let service =
                block_on(Builder::address(address.as_str()).name(DESTINATION).build()).unwrap();
            let served = service.clone();
            let served_calls = calls.clone();
            let messages = zbus::MessageStream::from(&service);
            std::thread::spawn(move || answer(&served, messages, &served_calls, highest));
            let client = RealtimeKit::on(
                &block_on(
                    Builder::address(address.as_str())
                        .method_timeout(CALL_TIMEOUT)
                        .build(),
                )
                .unwrap(),
            )
            .unwrap();
            Self {
                daemon,
                _service: service,
                client,
                calls,
                _turn: turn,
            }
        }
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    /// Needs the system bus with rtkit on it, so run it by hand on a desktop:
    /// `cargo test --lib real_rtkit -- --ignored`
    #[test]
    #[ignore = "asks the rtkit of the machine it runs on"]
    fn when_the_machine_has_rtkit_the_thread_becomes_real_time_through_it() {
        let rtkit = RealtimeKit::connect().unwrap();
        // SAFETY: gettid has no arguments and cannot fail
        let tid = unsafe { libc::syscall(libc::SYS_gettid) } as libc::pid_t;

        let given = rtkit.make_thread_realtime(tid, 20).unwrap();

        // SAFETY: reads the policy of a thread of this process
        let policy = unsafe { libc::sched_getscheduler(tid) };
        assert!(given >= 1, "given {given}");
        assert_eq!(policy & !libc::SCHED_RESET_ON_FORK, libc::SCHED_RR);
    }

    #[test]
    fn when_rtkit_allows_less_than_wanted_the_thread_gets_the_highest_it_allows() {
        let bus = Bus::start(20);

        let given = bus.client.make_thread_realtime(4321, 90).unwrap();

        assert_eq!(given, 20);
        assert_eq!(
            *bus.calls.lock().unwrap(),
            [("MakeThreadRealtime".to_string(), 4321, 20)]
        );
    }

    #[test]
    fn when_rtkit_allows_more_than_wanted_the_thread_gets_what_was_wanted() {
        let bus = Bus::start(99);

        let given = bus.client.make_thread_realtime(4321, 90).unwrap();

        assert_eq!(given, 90);
    }

    #[test]
    fn when_rtkit_allows_no_real_time_priority_it_is_not_asked_for_one() {
        let bus = Bus::start(0);

        let error = bus.client.make_thread_realtime(4321, 90).unwrap_err();

        assert!(error.contains("no real-time priority"), "{error}");
        assert!(bus.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn when_a_thread_is_made_high_priority_it_gets_the_lowest_nice_value_rtkit_allows() {
        let bus = Bus::start(20);

        let nice = bus.client.make_thread_high_priority(4321).unwrap();

        assert_eq!(nice, -15);
        assert_eq!(
            *bus.calls.lock().unwrap(),
            [("MakeThreadHighPriority".to_string(), 4321, -15)]
        );
    }
}
