//! RealtimeKit (rtkit): the system service that hands out real-time priority to ordinary
//! users on a desktop, over the system D-Bus.

use zbus::blocking::{Connection, Proxy};

const DESTINATION: &str = "org.freedesktop.RealtimeKit1";
const PATH: &str = "/org/freedesktop/RealtimeKit1";

pub(super) struct RealtimeKit {
    proxy: Proxy<'static>,
}

impl RealtimeKit {
    /// # Errors
    /// Why there is no rtkit to ask: no system bus, or nothing on it by that name.
    pub(super) fn connect() -> Result<Self, String> {
        let connection = Connection::system().map_err(|e| format!("no system bus: {e}"))?;
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

    #[test]
    fn when_there_is_no_system_bus_rtkit_cannot_be_reached_and_the_error_says_so() {
        std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/nonexistent/bus");

        let error = RealtimeKit::connect().err().expect("no bus to connect to");

        assert!(error.contains("no system bus"), "{error}");
    }
}
