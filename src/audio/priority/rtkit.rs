//! RealtimeKit (rtkit): the system service that hands out real-time priority to ordinary
//! users on a desktop, over the system D-Bus.

use std::time::Duration;

use zbus::blocking::connection::Builder;
use zbus::blocking::{Connection, Proxy};

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
        let connection = Builder::system()
            .and_then(|builder| builder.method_timeout(CALL_TIMEOUT).build())
            .map_err(|e| format!("no system bus: {e}"))?;
        Self::on(connection)
    }

    fn on(connection: Connection) -> Result<Self, String> {
        let proxy = Proxy::new_owned(connection, DESTINATION, PATH, DESTINATION)
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
        self.proxy
            .call::<_, _, ()>("MakeThreadRealtime", &(tid as u64, priority as u32))
            .map_err(|e| e.to_string())?;
        Ok(priority)
    }

    /// Gives the thread `tid` the lowest nice value rtkit allows. Returns it.
    ///
    /// # Errors
    /// Why rtkit did not.
    pub(super) fn make_thread_high_priority(&self, tid: libc::pid_t) -> Result<i32, String> {
        let nice: i32 = self.property("MinNiceLevel")?;
        self.proxy
            .call::<_, _, ()>("MakeThreadHighPriority", &(tid as u64, nice))
            .map_err(|e| e.to_string())?;
        Ok(nice)
    }

    fn property<T>(&self, name: &str) -> Result<T, String>
    where
        T: TryFrom<zbus::zvariant::OwnedValue>,
        T::Error: Into<zbus::Error>,
    {
        self.proxy.get_property(name).map_err(|e| e.to_string())
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
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};

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

    /// Answers as rtkit does, with the names, types and limits of its D-Bus interface
    struct StandInRtkit {
        calls: Calls,
        highest: i32,
    }

    #[zbus::interface(name = "org.freedesktop.RealtimeKit1")]
    impl StandInRtkit {
        #[zbus(property)]
        fn max_realtime_priority(&self) -> i32 {
            self.highest
        }

        #[zbus(property, name = "RTTimeUSecMax")]
        fn rt_time_usec_max(&self) -> i64 {
            200_000
        }

        #[zbus(property)]
        fn min_nice_level(&self) -> i32 {
            -15
        }

        fn make_thread_realtime(&self, thread: u64, priority: u32) {
            let call = ("MakeThreadRealtime".to_string(), thread, priority.into());
            self.calls.lock().unwrap().push(call);
        }

        fn make_thread_high_priority(&self, thread: u64, priority: i32) {
            let call = (
                "MakeThreadHighPriority".to_string(),
                thread,
                priority.into(),
            );
            self.calls.lock().unwrap().push(call);
        }
    }

    /// A bus of its own, with the stand-in on it, and a client connection to it
    struct Bus {
        daemon: Child,
        _service: Connection,
        client: RealtimeKit,
        calls: Calls,
    }

    impl Bus {
        fn start(highest: i32) -> Self {
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
            let service = Builder::address(address.as_str())
                .unwrap()
                .name(DESTINATION)
                .unwrap()
                .serve_at(
                    PATH,
                    StandInRtkit {
                        calls: calls.clone(),
                        highest,
                    },
                )
                .unwrap()
                .build()
                .unwrap();
            let client = RealtimeKit::on(
                Builder::address(address.as_str())
                    .unwrap()
                    .method_timeout(CALL_TIMEOUT)
                    .build()
                    .unwrap(),
            )
            .unwrap();
            Self {
                daemon,
                _service: service,
                client,
                calls,
            }
        }
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
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
