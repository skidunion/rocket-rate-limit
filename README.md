# rocket-rate-limit

A simple library based on [governor](https://docs.rs/governor/0.5.1/governor/index.html) for rate limiting in Rocket.rs.

It implements a `RateLimit` fairing that applies rate limiting based on routes.

## Quick setup

### Create an instance of `RateLimit`

```rust
use dashmap::DashMap;
use rocket_rate_limit::{IpKeyFilter, RateLimit, RateLimitConfig};
use rocket_rate_limit::governor::{RateLimiter, Quota};
use rocket_rate_limit::governor::state::InMemoryState;

let mut rate_limit: RateLimit<String, DashMap<String, InMemoryState>> = RateLimit::default(); 
```

We'll use the [dashmap](https://crates.io/crates/dashmap) crate to store our rate limit data.  

### Add a `RateLimitConfig` for every protected route

```rust
rate_limit.add("route_name", vec![
   RateLimitConfig::new(
      RateLimiter::keyed(
         Quota::with_period(Duration::from_millis(5000)).unwrap()
      ),
      Box::new(IpKeyFilter)
   ) 
]);
```

`route_name` is usually the name of the route function. 

These names are printed to the log on Rocket startup (section "Routes", route names are in parentheses).

You can also use the `rate_limit` macro for easier setup, which uses the default settings above:

```rust
rate_limit! {
    "route_name" => [
        {
            quota: Quota::with_period(Duration::from_millis(5000)).unwrap(),
            filter: IpKeyFilter
        }
    ]
}
```

### Attach the fairing

```rust
rocket.attach(rate_limit);
``` 

## Configuration

### Basics

For every protected route, you need to specify a _quota_ and a _filter_.

A _quota_ defines how fast the route can be accessed. See [governor docs](https://docs.rs/governor/0.5.1/governor/struct.Quota.html) for more info.

A _filter_ is a function that extracts a rate limit _key_ from the request. This key is then used to identify the user.

### Multiple configurations

It's possible to have multiple configurations for a single route. This can be used to apply different rate limit quotas 
to different users.

To distinguish between configurations, attach a _priority_ to them:

```rust
RateLimitConfig::new(...).priority(priority_number)
```

The rate limiter will first execute the config with the highest priority.

## Filters

A _filter_ is a struct that implements `KeyFilter`, that's used to extract a key for rate limiting. 
For example, the `IpKeyFilter` uses the user's IP address as the key.

Here's an example filter:

```rust
use rocket::Request;
use rocket_rate_limit::KeyFilter;

struct User {
    id: String
}

struct UserFilter;

#[rocket::async_trait]
impl KeyFilter<String> for UserFilter {
    async fn key(
        &self,
        req: &mut Request<'_>,
    ) -> Option<String> {
        Some(req.guard::<User>().succeeded()?.id)
    }
}
```

This loads an example `User` object from the request context, extracts the User's ID and uses it for rate limiting.

If a filter returns `None`, the `RateLimitConfig` which uses this filter is skipped. In the example above, if the user
hadn't been authenticated, no rate limits would be applied.

## Limitations

### A single `RateLimit` instance can only use one key type

In every `RateLimit<K, S>` you can only use filters of the `K` type.

In other words, you can't use a filter that extracts a `String` with a filter that extracts an `ObjectId` in the 
same rate limiter. You would have to refactor the second filter to use the `String` type, if you wish to use them both.

## License

rocket-rate-limit is Open Source software released under the [MIT License](LICENSE.md).