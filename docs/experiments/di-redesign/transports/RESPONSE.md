# Responses on the transport design

The design author's answers on the transport layer, in order, each with the user's sign-off when given.

---

## Naming exchange during the review

Received 2026-10-03, while the review was being written. The user asked about GraphQL's positioning
and about trait names; the author answered on naming. Not yet signed off. The GraphQL question has no
answer in this exchange.

dont wrry, code excution is enabled, check.........now, two thigns: graphql, the way it is positioned, does i acknlwoedge tha fact that grphawl can be paired with anyother thign? because as far a si know graphql does not necessarily need http, just that over http has astandardard,  thats why i am asking for this and the param trait, shont it be named extract? ...MiddlewareNext why not it be named next still i knwo itneceptor has next we can add or epxor alias for them if the user dont want to do module naming speicifally you get the point?

On the nested shape: yes, that was its purpose. It avoided repeating param in every variant. Both shapes are defensible. The flat one matches LookupError, which repeats key in every variant for the same reason: you can match ExtractError::Missing { param } in one pattern. I'd keep the flat shape for that consistency, and the param() accessor covers code that only wants the name.

Your naming rule
You're right on both counts, and the rule is better grounded than the symmetry argument I used for FromCall.

A trait is named after its predominant method. That's the dominant pattern in std: Clone::clone, Hash::hash, Drop::drop, Default::default, From::from, FromStr::from_str, IntoIterator::into_iter, Deref::deref, AsRef::as_ref, ToString::to_string. The early Rust naming conventions (RFC 344) point the same way: traits are preferably verbs, and a trait with one primary method shares its name.

When no single method fits, name the concept. That's Iterator (its method is next), Future (poll), Error, Read, and Write. The core's role traits (Guard, Interceptor, ErrorHandler) belong here: they name the role a type plays, not one method.

There's a matching rule for errors, also from std: an error that one operation produces is named after that operation. String::from_utf8 fails with FromUtf8Error, try_from with TryFromIntError, CString::into_string with IntoStringError, and parse with ParseIntError. An error shared by a whole family of operations is named after the domain: io::Error, and the core's LookupError.

Applying it
The inbound trait should be Extract, as you first suggested. Under this rule, FromCall::extract is wrong: the trait and its method disagree. There are two correct options:

FromCall::from_call, with an error named... FromCallError, which is awkward.
Extract::extract, with ExtractError, which aligns completely, like Clone::clone.
The second is clearly right. I chose FromCall for symmetry with FromContainer, but symmetry was a criterion I invented. Yours comes from std.

The outbound error should be IntoReplyError, not ReplyError. By the error rule, a failure of into_reply is IntoReplyError, as into_string fails with IntoStringError.

The core has one violation. FromContainer's method is read. It should be from_container, the way FromStr's is from_str. Its error stays LookupError, because that's a family error: dep, get, by_key and every resolver read produce it too, the way io::Error covers every I/O method.

The rest of what's built passes:

Trait	Method(s)	Verdict
Construct	construct → ConstructError	✓ fully aligned
Module	register, identity	✓ concept; no single method
Guard, Interceptor, ErrorHandler, Middleware	role methods	✓ roles
Timer, Server, Transport, Engine, Link	several methods	✓ concepts
Validate	validate	✓
Meta, Factory, Scope, Role, AllowedIn, HookCapable	markers or concepts	✓
FromContainer	read	✗ should be from_container
Classified	kind, public_message, details	✗ (see below)
Classified needs your decision. It's an adjective, and its predominant method is kind. The rule points to Classify::classify(&self) -> ErrorKind. But that collides with the free function fw_transport::classify(&BoxError) -> CallError, which does something different: it turns any error into a CallError. If the trait takes the name, the free function should move to CallError::from_error(&BoxError). That's a conversion constructor, which is where Rust usually puts such functions. Two things with the same name meaning different things would be worse than either name alone.

One more stutter, found while auditing: fw_http::HttpBackend becomes fw_http::Backend, by the rule you approved.

So the changes are:

Was	Now
FromCall::extract	Extract::extract → ExtractError (flat)
ReplyError	IntoReplyError
FromContainer::read (core, built)	FromContainer::from_container
Classified::kind + free classify(&BoxError)	Classify::classify + CallError::from_error(&BoxError), if you approve
HttpBackend	fw_http::Backend
if you think the Extract shoudl be a from or an inot because it conversta value or rather pull value from feel free to suggest ( i dunno the rule behaind from and into etc and try from and try into, but i guess the try varaints return and error and the others dont, but that makes sense only if the fallibel and non flaible variants exist toghether), otherwise, ..now on the lcassified, the name seems wwird by the way, what is your ecoendtion for bth its name nd the free funcion

From, Into, and the Try variants
You guessed the Try rule correctly. Try only appears where an infallible sibling exists: TryFrom exists because From does. A fallible conversion with no infallible sibling just returns a Result under the plain name. FromStr::from_str returns Result and isn't called TryFromStr. File::open isn't try_open.

The From / Into distinction is about direction:

From* means Self is built from a source: From<T>::from(t), FromStr::from_str(s), FromIterator::from_iter(it). The source is the input.
Into* means Self turns itself into a target, consuming itself: Into<T>::into(self), IntoIterator::into_iter(self).
Applied here:

The return value is Into. The handler's value is consumed and turned into the transport's reply, so IntoReply::into_reply(self) is right.
The parameter is From. The parameter is built from the call, the way a number is built from a string. The call isn't consumed, and only part of it is read. Into would point the wrong way. TryFrom would be wrong too, because std's TryFrom has a fixed, synchronous, by-value signature, and this has no infallible sibling.
That puts me in an awkward spot: it argues for FromCall::from_call, which I talked you out of last time. My objection then was that its error would have to be called FromCallError. Std shows that objection was wrong. FromStr::from_str fails with ParseIntError and ParseFloatError, named after the operation users think of ("parsing"), not after the trait. The core already does the same thing: FromContainer fails with LookupError. So the std-grounded and fully parallel design is:

Trait	Method	Error
FromContainer (core)	from_container	LookupError
FromCall<T>	from_call	ExtractError
IntoReply<T>	into_reply	IntoReplyError
Extract::extract also follows your method rule, so it isn't wrong. But FromCall says where the value comes from, and that matters here because a handler parameter can come from either source, the container or the call. The pair FromContainer / FromCall makes that visible in every diagnostic. I recommend FromCall.

Classified: the trait and the free function
It reads oddly because it's a past-participle adjective. Std mostly uses verbs or nouns for traits, and your method rule points straight at the fix.

The trait: Classify, with the method classify:

rust
pub trait Classify: Error + Send + Sync + 'static {
    fn classify(&self) -> ErrorKind;
    fn public_message(&self) -> Cow<'_, str> { self.to_string().into() }
    fn details(&self) -> Details { Details::default() }
}

#[derive(Debug, thiserror::Error, Classify)]
pub enum UserError {
    #[error("user not found")] #[classify(not_found)] NotFound,
}
impl Classify for UserError { fn classify(&self) -> ErrorKind { .. } } reads as a sentence, and the derive's helper attribute takes the same word.

The free function becomes conversions on CallError, split by what you're converting from:

From a typed domain error: impl<E: Classify> From<E> for CallError. Then ? works directly: inside a function returning Result<_, CallError>, a UserError converts itself. It's the same pattern anyhow uses. It compiles because CallError itself never implements Classify. If it did, the impl would collide with std's impl<T> From<T> for T. So CallError keeps an inherent kind() instead. That costs nothing, and the rule should be written in the docs.
From an arbitrary BoxError: CallError::from_boxed(err: BoxError) -> CallError. This is the recogniser: it walks the error and maps GuardRejected, PanicRecovered, ExtractError, LookupError::Construct and so on. It's a named constructor, like std's from_utf8 or from_raw, rather than a From impl, for two reasons. First, a From<BoxError> impl next to the blanket impl above would rely on the compiler proving that Box<dyn Error> can never implement Classify, and I wouldn't bet on coherence accepting that. Second, recognising types isn't a plain conversion, and a name that says so is clearer.
Both coherence claims should go into the probe crate before anything is built against them. The anyhow precedent makes me confident about the first, and the second is deliberately avoided.

Was	Now
Classified + #[kind(..)]	Classify::classify + #[derive(Classify)], #[classify(..)]
CallError::classified(e)	CallError::from(e), or just ?
free classify(&BoxError) -> CallError	CallError::from_boxed(BoxError)
Extract::extract (last message's suggestion)	FromCall::from_call
