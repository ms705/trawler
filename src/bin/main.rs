use clap::value_t_or_exit;
use clap::{App, Arg};
use futures_util::future::TryFutureExt;
use futures_util::future::{self, Either};
use lazy_static::lazy_static;
use tower_service::Service;
use trawler::{LobstersRequest, TrawlerRequest, Vote};

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::RwLock;
use std::task::{Context, Poll};
use std::time;

lazy_static! {
    static ref WEIGHTS: HashMap<String, isize> = HashMap::new();
}

lazy_static! {
    static ref SESSION_COOKIES: RwLock<HashMap<u32, cookie::CookieJar>> = RwLock::default();
}

#[derive(Clone)]
struct WebClient {
    prefix: url::Url,
    client: hyper::Client<hyper::client::HttpConnector>,
}

impl WebClient {
    fn new(prefix: &str) -> Self {
        let prefix = url::Url::parse(prefix).unwrap();
        let client = hyper::Client::new();
        WebClient { prefix, client }
    }

    fn get_cookie_for(
        &self,
        uid: u32,
    ) -> Pin<Box<dyn Future<Output = Result<cookie::CookieJar, hyper::Error>> + Send + 'static>>
    {
        {
            let cookies = SESSION_COOKIES.read().unwrap();
            if let Some(cookie) = cookies.get(&uid) {
                return Box::pin(future::ready(Ok(cookie.clone())));
            }
        }

        let url = hyper::Uri::from_str(self.prefix.join("login").unwrap().as_ref()).unwrap();
        let mut req = hyper::Request::post(url);
        let mut s = url::form_urlencoded::Serializer::new(String::new());
        s.append_pair("utf8", "✓");
        s.append_pair("email", &format!("user{}", uid));
        //s.append_pair("email", "test");
        s.append_pair("password", "test");
        s.append_pair("commit", "Login");
        s.append_pair("referer", self.prefix.as_ref());
        req.headers_mut().unwrap().insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        let req = req.body(s.finish().into()).unwrap();

        let req = self.client.request(req);
        Box::pin(async move {
            let res = req.await?;
            if res.status() != hyper::StatusCode::FOUND {
                let body = hyper::body::to_bytes(res.into_body()).await?;
                panic!(
                    "Failed to log in as user{}/test. Make sure to apply the patches!\n{}",
                    uid,
                    ::std::str::from_utf8(&*body).unwrap(),
                );
            }

            let mut cookie = cookie::CookieJar::new();
            for c in res.headers().get_all(hyper::header::SET_COOKIE) {
                let c = cookie::Cookie::parse(c.to_str().unwrap().to_string()).unwrap();
                cookie.add(c);
            }

            SESSION_COOKIES.write().unwrap().insert(uid, cookie.clone());
            Ok(cookie)
        })
    }
}

impl Service<bool> for WebClient {
    type Response = Self;
    type Error = hyper::Error;
    type Future = futures_util::future::Ready<Result<Self::Response, Self::Error>>;
    fn poll_ready(&mut self, _: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn call(&mut self, _: bool) -> Self::Future {
        eprintln!("note: did not re-create backend as lobsters client did not implement setup()");
        eprintln!("note: if priming fails, make sure you have run the lobsters setup scripts");
        futures_util::future::ready(Ok(self.clone()))
    }
}

impl Service<TrawlerRequest> for WebClient {
    type Response = ();
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>>;
    fn poll_ready(&mut self, _: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn call(
        &mut self,
        TrawlerRequest {
            user: uid,
            page: req,
            ..
        }: TrawlerRequest,
    ) -> Self::Future {
        let mut expected = hyper::StatusCode::OK;
        let mut req = match req {
            LobstersRequest::Frontpage => {
                let url = hyper::Uri::from_str(self.prefix.as_ref()).unwrap();
                hyper::Request::get(url).body(hyper::Body::empty()).unwrap()
            }
            LobstersRequest::Recent => {
                let url =
                    hyper::Uri::from_str(self.prefix.join("recent").unwrap().as_ref()).unwrap();
                hyper::Request::get(url).body(hyper::Body::empty()).unwrap()
            }
            LobstersRequest::Comments => {
                let url =
                    hyper::Uri::from_str(self.prefix.join("comments").unwrap().as_ref()).unwrap();
                hyper::Request::get(url).body(hyper::Body::empty()).unwrap()
            }
            LobstersRequest::User(uid) => {
                let url = hyper::Uri::from_str(
                    self.prefix
                        .join(&format!("u/user{}", uid))
                        .unwrap()
                        .as_ref(),
                )
                .unwrap();
                hyper::Request::get(url).body(hyper::Body::empty()).unwrap()
            }
            LobstersRequest::Login => {
                return Box::pin(self.get_cookie_for(uid.unwrap()).map_ok(|_| ()));
            }
            LobstersRequest::Logout => {
                /*
                let url =
                    hyper::Uri::from_str(self.prefix.join("logout").unwrap().as_ref()).unwrap();
                hyper::Request::new(hyper::Method::Post, url)
                */
                return Box::pin(future::ready(Ok(())));
            }
            LobstersRequest::Story(id) => {
                let url = hyper::Uri::from_str(
                    self.prefix
                        .join("s/")
                        .unwrap()
                        .join(::std::str::from_utf8(&id[..]).unwrap())
                        .unwrap()
                        .as_ref(),
                )
                .unwrap();
                hyper::Request::get(url).body(hyper::Body::empty()).unwrap()
            }
            LobstersRequest::StoryVote(story, v) => {
                let url = hyper::Uri::from_str(
                    self.prefix
                        .join(&format!(
                            "stories/{}/{}",
                            ::std::str::from_utf8(&story[..]).unwrap(),
                            match v {
                                Vote::Up => "upvote",
                                Vote::Down => "unvote",
                            }
                        ))
                        .unwrap()
                        .as_ref(),
                )
                .unwrap();
                hyper::Request::post(url)
                    .body(hyper::Body::empty())
                    .unwrap()
            }
            LobstersRequest::CommentVote(comment, v) => {
                let url = hyper::Uri::from_str(
                    self.prefix
                        .join(&format!(
                            "comments/{}/{}",
                            ::std::str::from_utf8(&comment[..]).unwrap(),
                            match v {
                                Vote::Up => "upvote",
                                Vote::Down => "unvote",
                            }
                        ))
                        .unwrap()
                        .as_ref(),
                )
                .unwrap();
                hyper::Request::post(url)
                    .body(hyper::Body::empty())
                    .unwrap()
            }
            LobstersRequest::Submit { id, title } => {
                expected = hyper::StatusCode::FOUND;

                let url =
                    hyper::Uri::from_str(self.prefix.join("stories").unwrap().as_ref()).unwrap();
                let mut req = hyper::Request::post(url);
                let mut s = url::form_urlencoded::Serializer::new(String::new());
                s.append_pair("commit", "Submit");
                s.append_pair("story[short_id]", ::std::str::from_utf8(&id[..]).unwrap());
                s.append_pair("story[tags_a][]", "test");
                s.append_pair("story[title]", &title);
                s.append_pair("story[description]", "to infinity");
                s.append_pair("utf8", "✓");
                req.headers_mut().unwrap().insert(
                    http::header::CONTENT_TYPE,
                    http::HeaderValue::from_static("application/x-www-form-urlencoded"),
                );
                req.body(s.finish().into()).unwrap()
            }
            LobstersRequest::Comment { id, story, parent } => {
                let url =
                    hyper::Uri::from_str(self.prefix.join("comments").unwrap().as_ref()).unwrap();
                let mut req = hyper::Request::post(url);
                let mut s = url::form_urlencoded::Serializer::new(String::new());
                s.append_pair("short_id", ::std::str::from_utf8(&id[..]).unwrap());
                s.append_pair("comment", "moar benchmarking");
                if let Some(parent) = parent {
                    s.append_pair(
                        "parent_comment_short_id",
                        ::std::str::from_utf8(&parent[..]).unwrap(),
                    );
                }
                s.append_pair("story_id", ::std::str::from_utf8(&story[..]).unwrap());
                s.append_pair("utf8", "✓");
                req.headers_mut().unwrap().insert(
                    http::header::CONTENT_TYPE,
                    http::HeaderValue::from_static("application/x-www-form-urlencoded"),
                );
                req.body(s.finish().into()).unwrap()
            }
        };

        let req = if let Some(uid) = uid {
            Either::Left(WebClient::get_cookie_for(self, uid).map_ok(move |cookies| {
                for c in cookies.iter() {
                    req.headers_mut().insert(
                        hyper::header::COOKIE,
                        hyper::header::HeaderValue::from_str(&format!("{}", c)).unwrap(),
                    );
                }
                req
            }))
        } else {
            Either::Right(future::ready(Ok(req)))
        };

        let client = self.client.clone();
        Box::pin(async move {
            let res = client.request(req.await?).await?;
            if res.status() != expected {
                let status = res.status();
                let body = hyper::body::to_bytes(res.into_body()).await?;
                panic!(
                    "{:?} status response. You probably forgot to prime.\n{}",
                    status,
                    ::std::str::from_utf8(&*body).unwrap(),
                );
            }
            Ok(())
        })
    }
}

impl trawler::AsyncShutdown for WebClient {
    type Future = futures_util::future::Ready<()>;
    fn shutdown(self) -> Self::Future {
        futures_util::future::ready(())
    }
}

fn main() {
    let args = App::new("trawler")
        .version("0.1")
        .about("Benchmark a lobste.rs Rails installation")
        .arg(
            Arg::with_name("reqscale")
                .long("reqscale")
                .takes_value(true)
                .default_value("1.0")
                .help("Scaling factor for generated load"),
        )
        .arg(
            Arg::with_name("datascale")
                .long("datascale")
                .takes_value(true)
                .default_value("1.0")
                .help("Scaling factor for data"),
        )
        .arg(
            Arg::with_name("prime")
                .long("prime")
                .help("Set if the backend must be primed with initial stories and comments."),
        )
        .arg(
            Arg::with_name("scale_everything")
                .long("scale_everything")
                .help("Set if you want to scale the data per user with the number of users."),
        )
        .arg(
            Arg::with_name("runtime")
                .short("r")
                .long("runtime")
                .takes_value(true)
                .default_value("30")
                .help("Benchmark runtime in seconds"),
        )
        .arg(
            Arg::with_name("histogram")
                .long("histogram")
                .help("Use file-based serialized HdrHistograms")
                .takes_value(true)
                .long_help(
                    "If the file already exists, the existing histogram is extended.\
                     There are two histograms, written out in order: \
                     sojourn and remote.",
                ),
        )
        .arg(
            Arg::with_name("prefix")
                .value_name("URL-PREFIX")
                .takes_value(true)
                .default_value("http://localhost:3000")
                .index(1),
        )
        .get_matches();

    let mut wl = trawler::WorkloadBuilder::default();
    wl.reqscale(value_t_or_exit!(args, "reqscale", f64))
        .datascale(value_t_or_exit!(args, "datascale", f64))
        .time(time::Duration::from_secs(value_t_or_exit!(
            args, "runtime", u64
        )));

    if let Some(h) = args.value_of("histogram") {
        wl.with_histogram(h);
    }

    wl.run(
        WebClient::new(args.value_of("prefix").unwrap()),
        args.is_present("prime"),
        &WEIGHTS,
        args.is_present("scale_everything"),
    );
}
