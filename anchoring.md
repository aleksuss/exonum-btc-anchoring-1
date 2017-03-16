# Этапы работы сервиса

## Этап подготовки

Выполняется во время инициализации работы блокчейна

### Генерация анкорящей конфигурации

Для каждого узла генерируется пара ключей `public_key, secret_key` в формате, принятом в `bitcoin-core`. По этим ключам создается `multisig` адрес и соответствующий ему `redeem_script`. После чего на адрес отсылается фундирующая транзакция. 
В конфигурацию сервиса записываются публичные ключи и фундирующая транзакция. Эта же транзакция записывается в качестве `lect` транзакции всем узлам, она же является корневой транзакцией для анкорящей цепочки. 

### Анкоринг первоначальной фундирующей транзакции

После запуска сети каждая нода смотрит на количество подтверждений фундирующей транзакции и если набирается достаточное количество записанное в параметре `utxo_confirmations`, то генерируется анкорящая транзакция и сеть начинает периодически анкорить блоки.

## Этап анкоринга

### Условия
* Если текущий и предыдущий `lect`ы указывают на текущий анкорящий адрес.
* Если нет предложение конфигурации с другим адресом

### Порядок работы

* Если текущий `lect` не указывает на текущую анкорящую высоту, то формируется `анкорящую транзакцию` по известным правилам и рассылается подпись за нее. Когда количество подписей за нее достигает `3/2+` она подписывается и отсылается `bitcoin узлу`, если у него еще нет такой в пуле. 

## Этап обновления списка ключей

### Условия
* Если есть предложение конфигурации с другим адресом
* Если адрес текущего `lect` совпадает с текущим, но отличается от предложенного

### Порядок работы

Аналогичен этапу анкоринга за исключением того, что `транзакция перевода` формируется только тогда, когда количество подтверждений `lect` превышает `utxo_confirmations`. После отправки транзакции узел переходит к этапу ожидания нового адреса.

## Этап ожидания нового адреса

### Условия
* Если адрес текущего `lect` не совпадает с актуальным адресом анкоринга
* Если адрес предыдущего `lect` отличается от адреса текущего, и количество подтверждений за текущий меньше `utxo_confirmations`
* Если есть предложение конфигурации с другим адресом, а адрес текущего `lect` совпадает с адресом в этом предположении.
 
 На этом этапе узел не осуществляет процедуру анкоринга.


# Обновление lect

Осуществляется при помощи вызова `listunspent` на адрес анкоринга и поиском подходящей анкорящей транзакции, которую можно использовать в качестве входа.

Каждая транзакция, которую возвращает этот вызов проверяется следующим образом:
* Проверяется, что транзакция является анкорящей и отправлена на проверяемый адрес
* Все предыдущие транзакции из цепочки являются анкорящими и тратят средства на известные анкорящие адреса. 
* Хотя бы одна транзакция из цепочки является известной нам `lect`

Если `listunspent` вернул пустой список, а предыдущий и текущий `lect` отправлены на разные адреса, это означает, что потерялась транзакция перехода на новый адрес и нужно откатиться на предыдущий `lect`

# Виды bitcoin транзакций

***Анкорящая*** 
 * Содержит 1 и более входов, первый из которых является предыдущим в цепочке анкоринга или же является корневой фундирующей транзакцией, остальные входы являются фундирующими транзакциями с количеством подтверждений большим, чем `utxo_confirmations`.
 * Содержит ровно два выхода. Первый выход переводит средства на анкорящий адрес, второй выход содержит высоту и хеш заанкоренного блока.

***Фундирующая***
 * Может содержать любое количество входов.
 * Может содержать любое количество выходов, главное, чтобы один из них тратил средства на анкорящий адрес