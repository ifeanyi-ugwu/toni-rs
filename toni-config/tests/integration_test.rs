use serial_test::serial;
use toni_config::{Config, ConfigError, ConfigModule, Environment};
use validator::Validate;

#[derive(Config, Clone)]
struct BasicConfig {
    #[env("TEST_VAR")]
    pub test_var: String,
}

#[derive(Config, Clone)]
struct ConfigWithDefaults {
    #[env("WITH_DEFAULT")]
    #[default(42u32)]
    pub with_default: u32,

    #[default("hello".to_string())]
    pub string_default: String,

    #[default(true)]
    pub bool_default: bool,
}

#[derive(Config, Clone)]
struct ConfigWithOptional {
    #[env("REQUIRED")]
    pub required: String,

    #[env("OPTIONAL")]
    pub optional: Option<String>,
}

#[derive(Config, Validate, Clone)]
struct ValidatedConfig {
    // Range is narrower than u16 so an in-range parse can still fail validation,
    // proving the check is validation and not a parse error.
    #[env("VALIDATED_PORT")]
    #[default(8080u16)]
    #[validate(range(min = 1024, max = 65535))]
    pub port: u16,
}

#[derive(Config, Clone)]
struct NestedInnerConfig {
    #[env("INNER_VALUE")]
    #[default(10u32)]
    pub value: u32,
}

#[derive(Config, Clone)]
struct NestedOuterConfig {
    #[env("OUTER_VALUE")]
    pub outer: String,

    #[nested]
    pub inner: NestedInnerConfig,
}

#[test]
#[serial]
fn test_basic_config() {
    std::env::set_var("TEST_VAR", "test_value");

    let config = ConfigModule::<BasicConfig>::from_env().unwrap();
    assert_eq!(config.get().test_var, "test_value");

    std::env::remove_var("TEST_VAR");
}

#[test]
#[serial]
fn test_missing_required_var() {
    std::env::remove_var("TEST_VAR");

    let result = ConfigModule::<BasicConfig>::from_env();
    assert!(result.is_err());

    if let Err(ConfigError::MissingEnvVar(var)) = result {
        assert_eq!(var, "TEST_VAR");
    } else {
        panic!("Expected MissingEnvVar error");
    }
}

#[test]
#[serial]
fn test_config_with_defaults() {
    std::env::remove_var("WITH_DEFAULT");
    std::env::remove_var("STRING_DEFAULT");
    std::env::remove_var("BOOL_DEFAULT");

    let config = ConfigModule::<ConfigWithDefaults>::from_env().unwrap();
    assert_eq!(config.get().with_default, 42);
    assert_eq!(config.get().string_default, "hello");
    assert_eq!(config.get().bool_default, true);
}

#[test]
#[serial]
fn test_default_override() {
    std::env::set_var("WITH_DEFAULT", "99");

    let config = ConfigModule::<ConfigWithDefaults>::from_env().unwrap();
    assert_eq!(config.get().with_default, 99);

    std::env::remove_var("WITH_DEFAULT");
}

#[test]
#[serial]
#[serial]
fn test_optional_present() {
    std::env::set_var("REQUIRED", "req");
    std::env::set_var("OPTIONAL", "opt");

    let config = ConfigModule::<ConfigWithOptional>::from_env().unwrap();
    assert_eq!(config.get().required, "req");
    assert_eq!(config.get().optional, Some("opt".to_string()));

    std::env::remove_var("REQUIRED");
    std::env::remove_var("OPTIONAL");
}

#[test]
#[serial]
fn test_optional_missing() {
    std::env::set_var("REQUIRED", "req");
    std::env::remove_var("OPTIONAL");

    let config = ConfigModule::<ConfigWithOptional>::from_env().unwrap();
    assert_eq!(config.get().required, "req");
    assert_eq!(config.get().optional, None);

    std::env::remove_var("REQUIRED");
}

#[test]
#[serial]
fn test_nested_config() {
    std::env::set_var("OUTER_VALUE", "outer");
    std::env::set_var("INNER_VALUE", "20");

    let config = ConfigModule::<NestedOuterConfig>::from_env().unwrap();
    assert_eq!(config.get().outer, "outer");
    assert_eq!(config.get().inner.value, 20);

    std::env::remove_var("OUTER_VALUE");
    std::env::remove_var("INNER_VALUE");
}

#[test]
#[serial]
fn test_nested_config_with_default() {
    std::env::set_var("OUTER_VALUE", "outer");
    std::env::remove_var("INNER_VALUE");

    let config = ConfigModule::<NestedOuterConfig>::from_env().unwrap();
    assert_eq!(config.get().outer, "outer");
    assert_eq!(config.get().inner.value, 10); // Default value

    std::env::remove_var("OUTER_VALUE");
}

#[test]
#[serial]
fn test_type_parsing() {
    std::env::set_var("WITH_DEFAULT", "123");

    let config = ConfigModule::<ConfigWithDefaults>::from_env().unwrap();
    assert_eq!(config.get().with_default, 123);

    std::env::remove_var("WITH_DEFAULT");
}

#[test]
#[serial]
fn test_invalid_type_parsing() {
    std::env::set_var("WITH_DEFAULT", "not_a_number");

    let result = ConfigModule::<ConfigWithDefaults>::from_env();
    assert!(result.is_err());

    if let Err(ConfigError::ParseError { key, .. }) = result {
        assert_eq!(key, "WITH_DEFAULT");
    } else {
        panic!("Expected ParseError");
    }

    std::env::remove_var("WITH_DEFAULT");
}

#[test]
#[serial]
fn test_validation_rejects_out_of_range() {
    std::env::set_var("VALIDATED_PORT", "80");

    let result = ConfigModule::<ValidatedConfig>::from_env();
    assert!(
        matches!(result, Err(ConfigError::ValidationError(_))),
        "port 80 is below the validated minimum of 1024 and must be rejected"
    );

    std::env::remove_var("VALIDATED_PORT");
}

#[test]
#[serial]
fn test_validation_accepts_in_range() {
    std::env::set_var("VALIDATED_PORT", "9000");

    let config = ConfigModule::<ValidatedConfig>::from_env().unwrap();
    assert_eq!(config.get().port, 9000);

    std::env::remove_var("VALIDATED_PORT");
}

#[test]
#[serial]
fn test_validation_uses_default_when_unset() {
    std::env::remove_var("VALIDATED_PORT");

    let config = ConfigModule::<ValidatedConfig>::from_env().unwrap();
    assert_eq!(config.get().port, 8080);
}

#[test]
#[serial]
fn test_environment_from_str() {
    assert!(matches!(
        Environment::from_str("development"),
        Environment::Development
    ));
    assert!(matches!(
        Environment::from_str("dev"),
        Environment::Development
    ));
    assert!(matches!(
        Environment::from_str("production"),
        Environment::Production
    ));
    assert!(matches!(
        Environment::from_str("prod"),
        Environment::Production
    ));
    assert!(matches!(Environment::from_str("test"), Environment::Test));

    if let Environment::Custom(name) = Environment::from_str("staging") {
        assert_eq!(name, "staging");
    } else {
        panic!("Expected Custom environment");
    }
}
