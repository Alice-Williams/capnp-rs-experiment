//! Calculator capabilities, callbacks, recursion, and promise pipelining.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::task::{Context, Poll, Wake, Waker};

use capnp_message::{ExclusiveArena, OwnedMessage, OwnedReadError, ReaderLimits};
use capnp_rpc::{
    BoxFuture, CapabilityList, LocalCall, LocalClient, LocalRequest, LocalResponse, LocalService,
    MessageFuture, PipelineBuilder, PipelineTransform, RpcError, TypedReader,
};
use capnp_schema::{CompiledSchema, DynamicError, DynamicInput};

use crate::calculator::expression::Which;
use crate::calculator::{
    Operator, calculator, call_params, call_results, def_function_params, def_function_results,
    evaluate_params, evaluate_results, expression, function, get_operator_params,
    get_operator_results, read_params, read_results, value,
};
use crate::{ExampleResult, calculator_schema};

const MESSAGE_WORD_LIMIT: u32 = 4096;
const CAPABILITY_LIMIT: usize = 16;

/// Observable results from the capability example.
#[derive(Clone, Debug, PartialEq)]
pub struct CalculatorRun {
    pub operator_result: f64,
    pub callback_result: f64,
    pub defined_function_result: f64,
    pub concurrent_results: [f64; 2],
    pub callback_calls: usize,
}

/// Runs operator, callback, recursive-expression, pipeline, and concurrency cases.
pub fn run() -> ExampleResult<CalculatorRun> {
    let schema = calculator_schema()?;
    let client = calculator_client(Arc::clone(&schema));

    let operator_call = client.get_operator(get_operator_request(&schema, Operator::Multiply)?);
    let operator = operator_call.pipeline.func().client()?;
    let operator_result = block_on(
        operator
            .call(function_request(&schema, &[6.0, 7.0])?)
            .response(),
    )?
    .value()?;
    let operator_response = block_on(operator_call.response())?;
    if operator_response.func()? != Some(0) {
        return Err(io::Error::other("operator response did not export capability zero").into());
    }

    let callback_calls = Arc::new(AtomicUsize::new(0));
    let callback = function_client(
        Arc::clone(&schema),
        CallbackFunction {
            calls: Arc::clone(&callback_calls),
        },
    );
    let (request, capabilities) = callback_evaluate_request(&schema, callback.local().clone())?;
    let evaluate_call = client.evaluate(LocalRequest::with_capabilities(request, capabilities));
    let promised_value = evaluate_call.pipeline.value().client()?;
    let response_barrier = Arc::new(Barrier::new(2));
    let response_thread = {
        let barrier = Arc::clone(&response_barrier);
        std::thread::spawn(move || {
            barrier.wait();
            block_on(evaluate_call.response())
        })
    };
    response_barrier.wait();
    let callback_result =
        block_on(promised_value.read(empty_read_request(&schema)?).response())?.value()?;
    let evaluate_response = response_thread
        .join()
        .map_err(|_| io::Error::other("calculator response thread panicked"))??;
    if evaluate_response.value()? != Some(0) {
        return Err(io::Error::other("evaluate response did not export capability zero").into());
    }

    let (define_request, define_caps) = define_function_request(&schema, callback.local().clone())?;
    let define_call =
        client.def_function(LocalRequest::with_capabilities(define_request, define_caps));
    let defined_function = define_call.pipeline.func().client()?;
    let defined_function_result = block_on(
        defined_function
            .call(function_request(&schema, &[41.0])?)
            .response(),
    )?
    .value()?;
    let define_response = block_on(define_call.response())?;
    if define_response.func()? != Some(0) {
        return Err(io::Error::other("defined-function response omitted its capability").into());
    }

    let concurrent_results = run_concurrently(&schema, &client)?;
    Ok(CalculatorRun {
        operator_result,
        callback_result,
        defined_function_result,
        concurrent_results,
        callback_calls: callback_calls.load(Ordering::SeqCst),
    })
}

#[derive(Clone)]
struct CalculatorService {
    schema: Arc<CompiledSchema>,
}

impl LocalService for CalculatorService {
    fn dispatch(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        _params: Arc<OwnedMessage>,
    ) -> MessageFuture {
        Box::pin(async move {
            Err(RpcError::Unimplemented {
                interface_id,
                method_id,
            })
        })
    }

    fn dispatch_request(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        request: LocalRequest,
    ) -> LocalCall {
        if interface_id != calculator::TYPE_ID {
            return failed_call(RpcError::UnknownInterface(interface_id));
        }
        match method_id {
            0 => self.evaluate(request),
            1 => self.define_function(request),
            2 => self.get_operator(request),
            _ => failed_call(RpcError::UnknownMethod {
                interface_id,
                method_id,
            }),
        }
    }
}

impl CalculatorService {
    fn get_operator(self: Arc<Self>, request: LocalRequest) -> LocalCall {
        let params = match get_operator_params::Reader::from_message(
            Arc::clone(&self.schema),
            Arc::clone(request.message()),
        ) {
            Ok(params) => params,
            Err(error) => return failed_call(error),
        };
        let operator = match params.op() {
            Ok(operator) => operator,
            Err(error) => return failed_call(error.into()),
        };
        let function = function_client(Arc::clone(&self.schema), OperatorFunction(operator));
        capability_call(
            Arc::clone(&self.schema),
            CapabilityResult::Operator,
            function.local().clone(),
        )
    }

    fn evaluate(self: Arc<Self>, request: LocalRequest) -> LocalCall {
        let params = match evaluate_params::Reader::from_message(
            Arc::clone(&self.schema),
            Arc::clone(request.message()),
        ) {
            Ok(params) => params,
            Err(error) => return failed_call(error),
        };
        let expression = match params.expression() {
            Ok(Some(expression)) => expression,
            Ok(None) => return failed_call(missing("evaluate expression")),
            Err(error) => return failed_call(error.into()),
        };
        let capabilities = request.capabilities().clone();
        let schema = Arc::clone(&self.schema);
        let (promise, resolver) = LocalClient::promise(Arc::clone(&schema));
        let mut pipeline = PipelineBuilder::default();
        if let Err(error) =
            pipeline.set_capability(PipelineTransform::root().pointer_field(0), promise)
        {
            return failed_call(error);
        }
        let response = Box::pin(async move {
            let result =
                evaluate_expression(expression, capabilities, Vec::new(), Arc::clone(&schema))
                    .await?;
            let value = value_client(Arc::clone(&schema), result);
            resolver.fulfill(value.local().clone())?;
            let message = evaluate_result_message(&schema)?;
            local_capability_response(message, value.local().clone())
        });
        match LocalCall::new(response).with_pipeline(pipeline) {
            Ok(call) => call,
            Err(error) => failed_call(error),
        }
    }

    fn define_function(self: Arc<Self>, request: LocalRequest) -> LocalCall {
        let params = match def_function_params::Reader::from_message(
            Arc::clone(&self.schema),
            Arc::clone(request.message()),
        ) {
            Ok(params) => params,
            Err(error) => return failed_call(error),
        };
        let parameter_count = match params.param_count() {
            Ok(value) if value >= 0 => value as usize,
            Ok(_) => return failed_call(missing("non-negative parameter count")),
            Err(error) => return failed_call(error.into()),
        };
        let body = match params.body() {
            Ok(Some(body)) => body,
            Ok(None) => return failed_call(missing("function body")),
            Err(error) => return failed_call(error.into()),
        };
        let function = function_client(
            Arc::clone(&self.schema),
            DefinedFunction {
                schema: Arc::clone(&self.schema),
                body,
                capabilities: request.capabilities().clone(),
                parameter_count,
            },
        );
        capability_call(
            Arc::clone(&self.schema),
            CapabilityResult::DefinedFunction,
            function.local().clone(),
        )
    }
}

#[derive(Clone, Copy)]
enum CapabilityResult {
    Operator,
    DefinedFunction,
}

fn capability_call(
    schema: Arc<CompiledSchema>,
    result: CapabilityResult,
    capability: LocalClient,
) -> LocalCall {
    let pipeline_capability = capability.clone();
    let response = Box::pin(async move {
        let message = match result {
            CapabilityResult::Operator => operator_result_message(&schema)?,
            CapabilityResult::DefinedFunction => defined_function_result_message(&schema)?,
        };
        local_capability_response(message, capability)
    });
    let mut pipeline = PipelineBuilder::default();
    if let Err(error) = pipeline.set_capability(
        PipelineTransform::root().pointer_field(0),
        pipeline_capability,
    ) {
        return failed_call(error);
    }
    match LocalCall::new(response).with_pipeline(pipeline) {
        Ok(call) => call,
        Err(error) => failed_call(error),
    }
}

fn local_capability_response(
    message: Arc<OwnedMessage>,
    capability: LocalClient,
) -> Result<LocalResponse, RpcError> {
    let capabilities = CapabilityList::from_clients([Some(capability)], CAPABILITY_LIMIT)?;
    Ok(LocalResponse::with_capabilities(message, capabilities))
}

fn operator_result_message(schema: &CompiledSchema) -> Result<Arc<OwnedMessage>, RpcError> {
    let mut arena = arena()?;
    get_operator_results::Builder::init_root(schema, &mut arena)?.set_func(0)?;
    owned(arena)
}

fn defined_function_result_message(schema: &CompiledSchema) -> Result<Arc<OwnedMessage>, RpcError> {
    let mut arena = arena()?;
    def_function_results::Builder::init_root(schema, &mut arena)?.set_func(0)?;
    owned(arena)
}

fn evaluate_result_message(schema: &CompiledSchema) -> Result<Arc<OwnedMessage>, RpcError> {
    let mut arena = arena()?;
    evaluate_results::Builder::init_root(schema, &mut arena)?.set_value(0)?;
    owned(arena)
}

#[derive(Clone, Copy)]
struct OperatorFunction(Operator);

impl function::Server for OperatorFunction {
    fn call(&self, params: call_params::Reader) -> MessageFuture {
        let operator = self.0;
        Box::pin(async move {
            let values = params
                .params()?
                .ok_or_else(|| missing("operator parameters"))?;
            if values.len()? != 2 {
                return Err(missing("exactly two operator parameters"));
            }
            let left = values.get(0)?;
            let right = values.get(1)?;
            let result = match operator {
                Operator::Add => left + right,
                Operator::Subtract => left - right,
                Operator::Multiply => left * right,
                Operator::Divide => left / right,
                Operator::Unrecognized(_) => return Err(missing("recognized operator")),
            };
            call_result_message(result)
        })
    }
}

#[derive(Clone)]
struct CallbackFunction {
    calls: Arc<AtomicUsize>,
}

impl function::Server for CallbackFunction {
    fn call(&self, params: call_params::Reader) -> MessageFuture {
        let calls = Arc::clone(&self.calls);
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            let values = params
                .params()?
                .ok_or_else(|| missing("callback parameters"))?;
            let mut total = 0.0;
            for index in 0..values.len()? {
                total += values.get(index)?;
            }
            call_result_message(total)
        })
    }
}

#[derive(Clone)]
struct DefinedFunction {
    schema: Arc<CompiledSchema>,
    body: expression::Reader,
    capabilities: CapabilityList,
    parameter_count: usize,
}

impl function::Server for DefinedFunction {
    fn call(&self, params: call_params::Reader) -> MessageFuture {
        let values = match params.params() {
            Ok(Some(values)) => values,
            Ok(None) => return Box::pin(async { Err(missing("defined-function parameters")) }),
            Err(error) => return Box::pin(async move { Err(error.into()) }),
        };
        let mut parameters = Vec::new();
        for index in 0..values.len().unwrap_or(0) {
            match values.get(index) {
                Ok(value) => parameters.push(value),
                Err(error) => return Box::pin(async move { Err(error.into()) }),
            }
        }
        if parameters.len() != self.parameter_count {
            return Box::pin(async { Err(missing("matching defined-function arity")) });
        }
        let body = self.body.clone();
        let capabilities = self.capabilities.clone();
        let schema = Arc::clone(&self.schema);
        Box::pin(async move {
            let value = evaluate_expression(body, capabilities, parameters, schema).await?;
            call_result_message(value)
        })
    }
}

#[derive(Clone, Copy)]
struct ValueService(f64);

impl value::Server for ValueService {
    fn read(&self, _params: read_params::Reader) -> MessageFuture {
        let value = self.0;
        Box::pin(async move { read_result_message(value) })
    }
}

fn evaluate_expression(
    expression: expression::Reader,
    capabilities: CapabilityList,
    parameters: Vec<f64>,
    schema: Arc<CompiledSchema>,
) -> BoxFuture<Result<f64, RpcError>> {
    Box::pin(async move {
        match expression.which()? {
            Which::Literal => Ok(expression.literal()?),
            Which::PreviousResult => {
                let index = expression
                    .previous_result()?
                    .ok_or_else(|| missing("previous-result capability"))?;
                let client = capability(&capabilities, index)?;
                Ok(value::Client::from_local(client)
                    .read(empty_read_request_rpc(&schema)?)
                    .response()
                    .await?
                    .value()?)
            }
            Which::Parameter => parameters
                .get(
                    usize::try_from(expression.parameter()?)
                        .map_err(|_| missing("parameter index"))?,
                )
                .copied()
                .ok_or_else(|| missing("parameter in range")),
            Which::Call => {
                let call = expression.call()?;
                let index = call
                    .function()?
                    .ok_or_else(|| missing("function capability"))?;
                let function = function::Client::from_local(capability(&capabilities, index)?);
                let args = call.params()?.ok_or_else(|| missing("call parameters"))?;
                let mut values = Vec::new();
                for index in 0..args.len()? {
                    values.push(
                        evaluate_expression(
                            args.get(index)?,
                            capabilities.clone(),
                            parameters.clone(),
                            Arc::clone(&schema),
                        )
                        .await?,
                    );
                }
                Ok(function
                    .call(function_request_rpc(&schema, &values)?)
                    .response()
                    .await?
                    .value()?)
            }
            Which::Unrecognized(_) => Err(missing("recognized expression variant")),
        }
    })
}

fn calculator_client(schema: Arc<CompiledSchema>) -> calculator::Client {
    let service = Arc::new(CalculatorService {
        schema: Arc::clone(&schema),
    });
    calculator::Client::from_local(LocalClient::new(schema, service))
}

fn function_client<S>(schema: Arc<CompiledSchema>, server: S) -> function::Client
where
    S: function::Server,
{
    let service = function::LocalServer::new(Arc::new(server), Arc::clone(&schema));
    function::Client::from_local(LocalClient::new(schema, Arc::new(service)))
}

fn value_client(schema: Arc<CompiledSchema>, result: f64) -> value::Client {
    let service = value::LocalServer::new(Arc::new(ValueService(result)), Arc::clone(&schema));
    value::Client::from_local(LocalClient::new(schema, Arc::new(service)))
}

fn capability(capabilities: &CapabilityList, index: u32) -> Result<LocalClient, RpcError> {
    capabilities
        .get(usize::try_from(index).map_err(|_| missing("capability index"))?)?
        .ok_or(RpcError::MissingCapability(index))
}

fn get_operator_request(
    schema: &CompiledSchema,
    operator: Operator,
) -> ExampleResult<Arc<OwnedMessage>> {
    let mut arena = ExclusiveArena::new(16, MESSAGE_WORD_LIMIT)?;
    get_operator_params::Builder::init_root(schema, &mut arena)?.set_op(operator)?;
    owned_example(arena)
}

fn function_request(schema: &CompiledSchema, values: &[f64]) -> ExampleResult<Arc<OwnedMessage>> {
    Ok(function_request_rpc(schema, values)?)
}

fn function_request_rpc(
    schema: &CompiledSchema,
    values: &[f64],
) -> Result<Arc<OwnedMessage>, RpcError> {
    let mut arena = arena()?;
    let mut params = call_params::Builder::init_root(schema, &mut arena)?;
    let length = u32::try_from(values.len()).map_err(|_| missing("bounded parameter list"))?;
    let mut list = params.init_params(length)?;
    for (index, value) in values.iter().copied().enumerate() {
        list.set(
            u32::try_from(index).map_err(|_| missing("parameter index"))?,
            DynamicInput::Float64(value),
        )?;
    }
    owned(arena)
}

fn empty_read_request(schema: &CompiledSchema) -> ExampleResult<Arc<OwnedMessage>> {
    Ok(empty_read_request_rpc(schema)?)
}

fn empty_read_request_rpc(schema: &CompiledSchema) -> Result<Arc<OwnedMessage>, RpcError> {
    let mut arena = arena()?;
    let _builder = read_params::Builder::init_root(schema, &mut arena)?;
    owned(arena)
}

fn callback_evaluate_request(
    schema: &CompiledSchema,
    callback: LocalClient,
) -> Result<(Arc<OwnedMessage>, CapabilityList), RpcError> {
    let mut arena = arena()?;
    let mut params = evaluate_params::Builder::init_root(schema, &mut arena)?;
    let mut expression = params.init_expression()?;
    let mut call = expression.call()?;
    call.set_function(0)?;
    let mut args = call.init_params(2)?;
    expression::Builder::from_dynamic(args.struct_element(0)?).set_literal(20.0)?;
    expression::Builder::from_dynamic(args.struct_element(1)?).set_literal(22.0)?;
    Ok((
        owned(arena)?,
        CapabilityList::from_clients([Some(callback)], CAPABILITY_LIMIT)?,
    ))
}

fn define_function_request(
    schema: &CompiledSchema,
    callback: LocalClient,
) -> Result<(Arc<OwnedMessage>, CapabilityList), RpcError> {
    let mut arena = arena()?;
    let mut params = def_function_params::Builder::init_root(schema, &mut arena)?;
    params.set_param_count(1)?;
    let mut body = params.init_body()?;
    let mut call = body.call()?;
    call.set_function(0)?;
    let mut args = call.init_params(2)?;
    expression::Builder::from_dynamic(args.struct_element(0)?).set_parameter(0)?;
    expression::Builder::from_dynamic(args.struct_element(1)?).set_literal(1.0)?;
    Ok((
        owned(arena)?,
        CapabilityList::from_clients([Some(callback)], CAPABILITY_LIMIT)?,
    ))
}

fn literal_evaluate_request(
    schema: &CompiledSchema,
    value: f64,
) -> Result<Arc<OwnedMessage>, RpcError> {
    let mut arena = arena()?;
    let mut params = evaluate_params::Builder::init_root(schema, &mut arena)?;
    params.init_expression()?.set_literal(value)?;
    owned(arena)
}

fn run_concurrently(
    schema: &Arc<CompiledSchema>,
    client: &calculator::Client,
) -> ExampleResult<[f64; 2]> {
    let mut threads = Vec::new();
    for input in [11.0, 31.0] {
        let schema = Arc::clone(schema);
        let client = client.clone();
        threads.push(std::thread::spawn(move || -> Result<f64, RpcError> {
            let call = client.evaluate(literal_evaluate_request(&schema, input)?);
            let value = call.pipeline.value().client()?;
            let response_thread = std::thread::spawn(move || block_on(call.response()));
            let result =
                block_on(value.read(empty_read_request_rpc(&schema)?).response())?.value()?;
            response_thread
                .join()
                .map_err(|_| missing("concurrent response thread"))??;
            Ok(result)
        }));
    }
    let first = threads
        .remove(0)
        .join()
        .map_err(|_| io::Error::other("first calculator worker panicked"))??;
    let second = threads
        .remove(0)
        .join()
        .map_err(|_| io::Error::other("second calculator worker panicked"))??;
    Ok([first, second])
}

fn call_result_message(value: f64) -> Result<Arc<OwnedMessage>, RpcError> {
    let schema = crate::calculator_schema().map_err(|_| missing("calculator schema"))?;
    let mut arena = arena()?;
    call_results::Builder::init_root(&schema, &mut arena)?.set_value(value)?;
    owned(arena)
}

fn read_result_message(value: f64) -> Result<Arc<OwnedMessage>, RpcError> {
    let schema = crate::calculator_schema().map_err(|_| missing("calculator schema"))?;
    let mut arena = arena()?;
    read_results::Builder::init_root(&schema, &mut arena)?.set_value(value)?;
    owned(arena)
}

fn arena() -> Result<ExclusiveArena, RpcError> {
    ExclusiveArena::new(16, MESSAGE_WORD_LIMIT)
        .map_err(DynamicError::from)
        .map_err(RpcError::from)
}

fn owned(arena: ExclusiveArena) -> Result<Arc<OwnedMessage>, RpcError> {
    OwnedMessage::new(arena.into_segments(), ReaderLimits::default())
        .map_err(OwnedReadError::Validation)
        .map_err(RpcError::Message)
}

fn owned_example(arena: ExclusiveArena) -> ExampleResult<Arc<OwnedMessage>> {
    Ok(OwnedMessage::new(
        arena.into_segments(),
        ReaderLimits::default(),
    )?)
}

fn missing(expected: &'static str) -> RpcError {
    RpcError::Dynamic(DynamicError::TypeMismatch { expected })
}

fn failed_call(error: RpcError) -> LocalCall {
    LocalCall::new(Box::pin(async move { Err(error) }))
}

struct ThreadWake(std::thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match Pin::as_mut(&mut future).poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculator_exercises_capabilities_and_pipelines() -> ExampleResult<()> {
        let result = run()?;
        assert_eq!(result.operator_result, 42.0);
        assert_eq!(result.callback_result, 42.0);
        assert_eq!(result.defined_function_result, 42.0);
        assert_eq!(result.concurrent_results, [11.0, 31.0]);
        assert_eq!(result.callback_calls, 2);
        Ok(())
    }
}
