pub extern crate governor;

use governor::clock::{Clock, DefaultClock};
use governor::state::keyed::KeyedStateStore;
use governor::RateLimiter;
use rocket::fairing::{Fairing, Info, Kind};
use rocket::http::uri::Origin;
use rocket::http::{Header, Status};
use rocket::{Data, Request, Response, Route};
use std::collections::HashMap;
use std::hash::Hash;

#[macro_export]
macro_rules! rate_limit {
  {
    $(
        $name:literal => [
          $( { quota: $quota:expr, filter: $filter:ident } ), +
        ]
    ), +
  } => {
    let mut rate_limit = ::rocket_rate_limit::RateLimit::default();

    $(
      rate_limit.add($name, vec![
        $(
          ::rocket_rate_limit::RateLimitConfig::new(
            ::rocket_rate_limit::governor::RateLimiter::keyed($quota),
            Box::new($filter)
          )
        )+
      ]);
    )+

    rate_limit
  }
}

/// URI that rate limited requests get redirected to.
///
/// This is a magic value which allows the rate limiter to work.
/// Please don't use the same path in your routes.
///
const DUMMY_HANDLER_URI: &'static str =
    "/rate-limiter-handler-ZoIGMRpd2xPAOawvWc2T8m9Hs33E3kX8";

/// Dynamically extract rate-limit keys from requests.
///
/// This allows for custom key implementations. For example:
///
/// ```no_run
///# use rocket::Request;
///# use rocket_rate_limit::KeyFilter;
/// struct User {
///     id: String
/// }
///
/// struct UserFilter;
///
/// #[rocket::async_trait]
/// impl KeyFilter<String> for UserFilter {
///     async fn key(
///         &self,
///         req: &mut Request<'_>,
///     ) -> Option<String> {
///         Some(req.guard::<User>().succeeded()?.id)
///     }
/// }
/// ```
///
#[rocket::async_trait]
pub trait KeyFilter<K> {
    /// Extracts a key for the rate limiter.
    ///
    /// If a `None` is returned, the [RateLimiterConfig] is skipped.
    ///
    async fn key(&self, req: &Request<'_>) -> Option<K>;
}

pub struct IpKeyFilter;

#[rocket::async_trait]
impl KeyFilter<String> for IpKeyFilter {
    async fn key(&self, req: &Request<'_>) -> Option<String> {
        req.client_ip().map(|ip| ip.to_string())
    }
}

#[derive(Default)]
pub struct RateLimit<K, S>
where
    K: Eq + Clone + Hash,
    S: KeyedStateStore<K>,
{
    configs: HashMap<String, Vec<RateLimitConfig<K, S>>>,
    clock: DefaultClock,
}

impl<K, S> RateLimit<K, S>
where
    K: Eq + Clone + Hash,
    S: KeyedStateStore<K>,
{
    pub fn new(
        configs: HashMap<String, Vec<RateLimitConfig<K, S>>>,
    ) -> Self {
        RateLimit {
            configs,
            clock: DefaultClock::default(),
        }
    }

    pub fn add<R, I>(&mut self, route_name: R, items_iter: I)
    where
        R: AsRef<str>,
        I: IntoIterator<Item = RateLimitConfig<K, S>>,
    {
        let route_name = route_name.as_ref();

        if let Some(ref mut items) = self.configs.get_mut(route_name)
        {
            items.extend(items_iter);

            // Sort in reverse order by priority.
            items.sort_by(|a, b| b.priority.cmp(&a.priority));
        } else {
            self.configs.insert(
                route_name.to_string(),
                items_iter.into_iter().collect(),
            );
        }
    }

    async fn check_rate_limit(
        &self,
        req: &Request<'_>,
        route: Option<&Route>,
    ) -> RateLimitResult {
        let configs = route
            .and_then(|route| route.name.as_ref())
            .and_then(|name| self.configs.get(name.as_ref()))?;

        // Check if the context matches the mode.
        for cfg in configs {
            let result =
                cfg.filter.key(req).await.and_then(|key| {
                    cfg.limiter.check_key(&key).err()
                });

            if let Some(err_outcome) = result {
                return Some(RateLimitResponse {
                    retry_after: err_outcome
                        .wait_time_from(self.clock.now())
                        .as_millis(),
                });
            }
        }

        None
    }

    fn apply_rate_limit(
        &self,
        res: &mut Response<'_>,
        rate_limit: &RateLimitResponse,
    ) {
        use std::io::Cursor;

        res.set_status(Status::TooManyRequests);

        // Add rate-limit headers.
        res.set_header(Header::new(
            "Retry-After",
            rate_limit.retry_after.to_string(),
        ));

        // Remove the body (set empty body with 0 length).
        res.set_sized_body(0, Cursor::new(String::new()));
    }
}

#[derive(Clone, Copy)]
struct RateLimitResponse {
    retry_after: u128,
}

type RateLimitResult = Option<RateLimitResponse>;

pub struct RateLimitConfig<K, S>
where
    K: Eq + Clone + Hash,
    S: KeyedStateStore<K>,
{
    limiter: RateLimiter<K, S, DefaultClock>,
    filter: Box<dyn KeyFilter<K> + Send + Sync>,
    priority: u32,
}

impl<K, S> RateLimitConfig<K, S>
where
    K: Eq + Clone + Hash,
    S: KeyedStateStore<K>,
{
    pub fn new(
        limiter: RateLimiter<K, S, DefaultClock>,
        filter: Box<dyn KeyFilter<K> + Send + Sync>,
    ) -> Self {
        RateLimitConfig {
            limiter,
            filter,
            priority: 0,
        }
    }

    pub fn priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }
}

#[rocket::async_trait]
impl<K, S> Fairing for RateLimit<K, S>
where
    S: KeyedStateStore<K> + Send + Sync + 'static,
    K: Eq + Clone + Hash + Send + Sync + 'static,
{
    fn info(&self) -> Info {
        Info {
            name: "Rate Limit",
            kind: Kind::Request | Kind::Response,
        }
    }

    async fn on_request(
        &self,
        req: &mut Request<'_>,
        _data: &mut Data<'_>,
    ) {
        let route =
            req.rocket().routes().find(|route| route.matches(req));

        let result = self.check_rate_limit(req, route).await;

        if let Some(rate_limit) = result {
            let uri =
                Origin::parse_owned(format!("{}", DUMMY_HANDLER_URI))
                    .expect("valid redirect uri");

            req.set_uri(uri);

            req.local_cache(|| Some(rate_limit));
        }
    }

    async fn on_response<'r>(
        &self,
        req: &'r Request<'_>,
        res: &mut Response<'r>,
    ) {
        if req.uri().path() != DUMMY_HANDLER_URI {
            return;
        }

        let dummy_result: RateLimitResult = None;
        let result = req.local_cache(|| dummy_result);

        if let Some(rate_limit) = result {
            self.apply_rate_limit(res, rate_limit);
        }
    }
}
